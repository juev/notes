import Foundation

/// A replacement in a text, in UTF-16 units, the way `splice_utf16` of the core takes it.
struct TextEdit: Equatable {
    var position: Int
    var deleted: Int
    var inserted: String

    var insertedUnits: Int { inserted.utf16.count }

    /// The smallest edit that turns `old` into `new`: what the two do not share at their ends. Used
    /// for a text that changed under the editor (S40), so that the rest of it stays in place.
    static func between(_ old: String, _ new: String) -> TextEdit? {
        let a = Array(old.utf16), b = Array(new.utf16)
        if a == b { return nil }
        var start = 0
        while start < a.count, start < b.count, a[start] == b[start] { start += 1 }
        // Not between the two halves of a surrogate pair.
        if start > 0, start < a.count, UTF16.isTrailSurrogate(a[start]) { start -= 1 }
        var tail = 0
        while tail < a.count - start, tail < b.count - start, a[a.count - 1 - tail] == b[b.count - 1 - tail] { tail += 1 }
        if tail > 0, UTF16.isTrailSurrogate(a[a.count - tail]) { tail -= 1 }
        return TextEdit(position: start, deleted: a.count - start - tail, inserted: String(decoding: b[start..<(b.count - tail)], as: UTF16.self))
    }

    /// The text after this edit.
    func applied(to text: String) -> String {
        var units = Array(text.utf16)
        units.replaceSubrange(position..<(position + deleted), with: Array(inserted.utf16))
        return String(decoding: units, as: UTF16.self)
    }

    /// The same edit for a text that `other` has changed meanwhile (S3): both stay.
    func rebased(over other: TextEdit) -> TextEdit {
        let start = other.moved(position)
        return TextEdit(position: start, deleted: max(0, other.moved(position + deleted) - start), inserted: inserted)
    }

    /// Where a position of the text before this edit is in the text after it: behind the edit it moves
    /// with the text, inside what was replaced it goes to the end of the new text (S40).
    func moved(_ offset: Int) -> Int {
        if offset <= position { return offset }
        if offset >= position + deleted { return offset - deleted + insertedUnits }
        return position + insertedUnits
    }
}

/// Puts edits made in a row together, so that the core opens its document once for a word and not
/// once for every key: a keystroke at the end of what is pending joins it, as does a backspace
/// over it. Any other edit starts a new one.
struct EditCoalescer {
    private(set) var pending: TextEdit?

    /// Takes in an edit; returns what must reach the core before it, when it does not join.
    mutating func add(_ edit: TextEdit) -> TextEdit? {
        guard var current = pending else {
            pending = edit
            return nil
        }
        let end = current.position + current.insertedUnits
        if edit.position == end, edit.deleted == 0 {
            current.inserted += edit.inserted
            pending = current
            return nil
        }
        if edit.inserted.isEmpty, edit.position + edit.deleted == end {
            if edit.deleted <= current.insertedUnits {
                let units = Array(current.inserted.utf16).dropLast(edit.deleted)
                current.inserted = String(decoding: units, as: UTF16.self)
                pending = current
                return nil
            }
            if current.inserted.isEmpty {
                pending = TextEdit(position: edit.position, deleted: current.deleted + edit.deleted, inserted: "")
                return nil
            }
        }
        pending = edit
        return current
    }

    /// What is pending, and nothing is any longer.
    mutating func take() -> TextEdit? {
        defer { pending = nil }
        return pending
    }
}

/// E11–E13, E16: whether the source of a span shows: while the cursor or the selection touches its
/// reveal range, its ends included, and the view has the keyboard.
enum Reveal {
    static func touches(selection: NSRange, start: Int, end: Int) -> Bool {
        selection.location <= end && start <= selection.location + selection.length
    }
}
