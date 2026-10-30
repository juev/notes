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
