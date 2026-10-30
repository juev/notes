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
