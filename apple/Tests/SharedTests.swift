import XCTest

/// The core through the bindings, the way the app reaches it.
final class CoreTests: XCTestCase {
    func testAStoreOpensThroughTheBindingsAndKeepsANote() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = try Store.open(dir: dir.path)
        let note = try store.createNote(new: NewNote(title: "Заметка", folderId: nil, tags: [], text: "**текст**"))
        XCTAssertEqual(try store.getText(id: note.id), "**текст**")
        XCTAssertEqual(try store.notes(scope: .all).map(\.preview), ["текст"])
    }
}

/// R3, R16, R25: what the list of notes shows and in what order.
final class ViewDisplayTests: XCTestCase {
    private func note(_ id: String, title: String = "", preview: String = "", pinned: Bool = false, edited: UInt64 = 0, created: UInt64 = 0) -> Note {
        Note(id: id, title: title, folderId: nil, tags: [], pinned: pinned, starred: false, archived: false, locked: false, deleted: false,
             createdMs: created, editedMs: edited, deletedMs: nil, attachments: 0, preview: preview)
    }

    func testTheTitleOfANoteFallsBackToItsFirstLineAndThenToUntitled() {
        XCTAssertEqual(note("a", title: "Заголовок", preview: "текст").shownTitle, "Заголовок")
        XCTAssertEqual(note("a", title: "  ", preview: "первая строка").shownTitle, "первая строка")
        XCTAssertEqual(note("a").shownTitle, "Untitled")
    }

    func testPinnedNotesComeFirstUnderAnySort() {
        let notes = [note("old", edited: 1), note("new", edited: 3), note("pinned", pinned: true, edited: 2)]
        let edited = ViewDisplay.sorted(notes, by: FolderSort(by: .edited, descending: true))
        XCTAssertEqual(edited.map(\.id), ["pinned", "new", "old"])
        let ascending = ViewDisplay.sorted(notes, by: FolderSort(by: .edited, descending: false))
        XCTAssertEqual(ascending.map(\.id), ["pinned", "old", "new"])
    }

    func testTitlesSortAsPeopleReadThem() {
        let notes = [note("b", title: "Заметка 10"), note("a", title: "Заметка 2")]
        let sorted = ViewDisplay.sorted(notes, by: FolderSort(by: .title, descending: false))
        XCTAssertEqual(sorted.map(\.id), ["a", "b"])
    }

    func testDisplayKeepsTheSortOfAFolder() {
        let display = ViewDisplay(sort: FolderSort(by: .created, descending: false), showPreview: false, showDate: true)
        XCTAssertEqual(display.sort, FolderSort(by: .created, descending: false))
        XCTAssertFalse(display.showPreview)
    }
}

/// E11: the source of a span shows while the selection touches its reveal range, ends included.
final class RevealTests: XCTestCase {
    func testTheEndsOfTheRangeCount() {
        // `a **b** c`: the markers of the bold span reveal over 2...7.
        XCTAssertFalse(Reveal.touches(selection: NSRange(location: 1, length: 0), start: 2, end: 7))
        XCTAssertTrue(Reveal.touches(selection: NSRange(location: 2, length: 0), start: 2, end: 7))
        XCTAssertTrue(Reveal.touches(selection: NSRange(location: 7, length: 0), start: 2, end: 7))
        XCTAssertFalse(Reveal.touches(selection: NSRange(location: 8, length: 0), start: 2, end: 7))
        XCTAssertTrue(Reveal.touches(selection: NSRange(location: 0, length: 3), start: 2, end: 7))
    }
}

/// R4, S40: what typing sends to the core, and what comes in from another device.
final class TextEditTests: XCTestCase {
    func testKeystrokesInARowJoinOneEdit() {
        var edits = EditCoalescer()
        XCTAssertNil(edits.add(TextEdit(position: 5, deleted: 0, inserted: "с")))
        XCTAssertNil(edits.add(TextEdit(position: 6, deleted: 0, inserted: "л")))
        XCTAssertNil(edits.add(TextEdit(position: 7, deleted: 0, inserted: "😀")))
        XCTAssertNil(edits.add(TextEdit(position: 7, deleted: 2, inserted: "")), "backspace over the emoji")
        XCTAssertEqual(edits.take(), TextEdit(position: 5, deleted: 0, inserted: "сл"))
        XCTAssertNil(edits.take())
    }

    func testAnEditElsewhereSendsWhatWaits() {
        var edits = EditCoalescer()
        XCTAssertNil(edits.add(TextEdit(position: 5, deleted: 0, inserted: "а")))
        XCTAssertEqual(edits.add(TextEdit(position: 0, deleted: 0, inserted: "б")), TextEdit(position: 5, deleted: 0, inserted: "а"))
        XCTAssertEqual(edits.take(), TextEdit(position: 0, deleted: 0, inserted: "б"))
    }

    func testBackspacesInARowJoinOneDeletion() {
        var edits = EditCoalescer()
        XCTAssertNil(edits.add(TextEdit(position: 4, deleted: 1, inserted: "")))
        XCTAssertNil(edits.add(TextEdit(position: 3, deleted: 1, inserted: "")))
        XCTAssertEqual(edits.take(), TextEdit(position: 3, deleted: 2, inserted: ""))
    }

    func testTheDifferenceLeavesWhatTheTextsShare() {
        XCTAssertNil(TextEdit.between("а", "а"))
        XCTAssertEqual(TextEdit.between("начало конец", "начало новое конец"), TextEdit(position: 7, deleted: 0, inserted: "новое "))
        XCTAssertEqual(TextEdit.between("abc", "ac"), TextEdit(position: 1, deleted: 1, inserted: ""))
        // Two emoji that share a first half are not cut between their halves.
        XCTAssertEqual(TextEdit.between("😀", "😁"), TextEdit(position: 0, deleted: 2, inserted: "😁"))
    }

    func testPositionsMoveWithTheText() {
        let edit = TextEdit(position: 5, deleted: 2, inserted: "новое")
        XCTAssertEqual(edit.moved(3), 3)
        XCTAssertEqual(edit.moved(5), 5)
        XCTAssertEqual(edit.moved(6), 10, "inside what was replaced: to the end of the new text")
        XCTAssertEqual(edit.moved(9), 12)
    }
}
