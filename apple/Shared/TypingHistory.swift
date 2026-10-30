import Foundation

/// One change of the text by the user, with what it took out, so that it can be taken back.
struct TypedEdit: Equatable {
    var position: Int
    var removed: String
    var inserted: String

    var removedUnits: Int { removed.utf16.count }
    var insertedUnits: Int { inserted.utf16.count }
    /// The edit that takes this one back.
    var inverse: TextEdit { TextEdit(position: position, deleted: insertedUnits, inserted: removed) }
    /// The edit itself, to apply it again.
    var forward: TextEdit { TextEdit(position: position, deleted: removedUnits, inserted: inserted) }
}

/// E25: the undo of what the user typed in the open note, in steps separated by pauses and by
/// formatting actions. Edits from other devices are not in it: they move the positions of what
/// is, so that undo takes back the typing and leaves them. It lives as long as the note stays open.
struct TypingHistory {
    /// A pause this long starts a new step.
    static let pause: TimeInterval = 1

    private(set) var undoStack: [[TypedEdit]] = []
    private(set) var redoStack: [[TypedEdit]] = []
    private var lastTime: Date?
    private var open = false

    /// Takes in an edit. True when it starts a new step, which the undo manager has to hear about.
    mutating func record(_ edit: TypedEdit, at time: Date) -> Bool {
        redoStack.removeAll()
        defer { lastTime = time }
        if open, let lastTime, time.timeIntervalSince(lastTime) < Self.pause, !undoStack.isEmpty {
            undoStack[undoStack.count - 1].append(edit)
            return false
        }
        undoStack.append([edit])
        open = true
        return true
    }

    /// The next edit starts a new step: a formatting action stands on its own.
    mutating func close() { open = false }

    /// The newest step, moved to the redo side: its edits are taken back from the last to the first.
    mutating func popUndo() -> [TypedEdit]? {
        open = false
        guard let step = undoStack.popLast() else { return nil }
        redoStack.append(step)
        return step
    }

    /// The step undone last, moved back: its edits are applied again from the first to the last.
    mutating func popRedo() -> [TypedEdit]? {
        open = false
        guard let step = redoStack.popLast() else { return nil }
        undoStack.append(step)
        return step
    }

    /// An edit from another device: every position of the history moves with the text. The edit
    /// is in the coordinates of the text now, the newest typed edit is one step back from them, and
    /// so on. What it overlaps can no longer be taken back, nor can anything older than that.
    mutating func outside(_ edit: TextEdit) {
        redoStack.removeAll()
        open = false
        var other = edit
        var stack = undoStack
        for s in stack.indices.reversed() {
            for e in stack[s].indices.reversed() {
                let typed = stack[s][e]
                if other.position >= typed.position + typed.insertedUnits {
                    other.position += typed.removedUnits - typed.insertedUnits
                } else if other.position + other.deleted <= typed.position {
                    stack[s][e].position += other.insertedUnits - other.deleted
                } else {
                    stack.removeFirst(s + 1)
                    undoStack = stack
                    return
                }
            }
        }
        undoStack = stack
    }
}
