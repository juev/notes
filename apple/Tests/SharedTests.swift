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

/// E25: the undo of typing in steps, with edits from other devices moving its positions.
final class TypingHistoryTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1000)

    func testAPauseAndAFormatStartANewStep() {
        var history = TypingHistory()
        XCTAssertTrue(history.record(TypedEdit(position: 0, removed: "", inserted: "а"), at: start))
        XCTAssertFalse(history.record(TypedEdit(position: 1, removed: "", inserted: "б"), at: start + 0.5))
        XCTAssertTrue(history.record(TypedEdit(position: 2, removed: "", inserted: "в"), at: start + 2))
        history.close()
        XCTAssertTrue(history.record(TypedEdit(position: 3, removed: "", inserted: "г"), at: start + 2.1))
        XCTAssertEqual(history.undoStack.map(\.count), [2, 1, 1])
    }

    func testAnEditBeforeTheTypedTextMovesIt() {
        // The scenario of the specification: A types, B's edit arrives, undo takes back A's typing.
        var history = TypingHistory()
        _ = history.record(TypedEdit(position: 0, removed: "", inserted: "свой текст"), at: start)
        history.outside(TextEdit(position: 0, deleted: 0, inserted: "ЧУЖОЕ"))
        XCTAssertEqual(history.popUndo()?.first?.inverse, TextEdit(position: 5, deleted: 10, inserted: ""))
    }

    func testAnEditAfterTheTypedTextLeavesIt() {
        var history = TypingHistory()
        _ = history.record(TypedEdit(position: 0, removed: "", inserted: "аб"), at: start)
        _ = history.record(TypedEdit(position: 2, removed: "", inserted: "вг"), at: start + 0.1)
        history.outside(TextEdit(position: 6, deleted: 0, inserted: "ЧУЖОЕ"))
        XCTAssertEqual(history.popUndo()?.map(\.position), [0, 2])
    }

    func testAnEditInsideTheTypedTextEndsWhatCanBeTakenBack() {
        var history = TypingHistory()
        _ = history.record(TypedEdit(position: 0, removed: "", inserted: "старое"), at: start)
        _ = history.record(TypedEdit(position: 6, removed: "", inserted: "новое"), at: start + 5)
        history.outside(TextEdit(position: 2, deleted: 1, inserted: "Х"))
        XCTAssertEqual(history.undoStack.count, 1, "the newest step stands, the one it overlaps is gone")
        XCTAssertEqual(history.undoStack.first?.first?.inserted, "новое")
    }

    func testRedoAppliesAgainAndNewTypingEndsIt() {
        var history = TypingHistory()
        _ = history.record(TypedEdit(position: 0, removed: "x", inserted: "y"), at: start)
        XCTAssertNotNil(history.popUndo())
        XCTAssertEqual(history.popRedo()?.first?.forward, TextEdit(position: 0, deleted: 1, inserted: "y"))
        XCTAssertNotNil(history.popUndo())
        _ = history.record(TypedEdit(position: 0, removed: "", inserted: "z"), at: start + 5)
        XCTAssertNil(history.popRedo())
    }
}

/// E41–E48: what the settings of typography come to.
final class TypographyTests: XCTestCase {
    func testTheSizeStaysInItsRange() {
        XCTAssertEqual(Typography.size(saved: nil, system: 13), 13)
        XCTAssertEqual(Typography.size(saved: 40, system: 13), 28)
        XCTAssertEqual(Typography.size(saved: 5, system: 13), 12)
    }

    func testTheStepsOfTheSettings() {
        XCTAssertEqual(Typography.lineHeights.first, 1.2)
        XCTAssertEqual(Typography.lineHeights.last, 2.0)
        XCTAssertEqual(Typography.lineHeights.count, 9)
        XCTAssertEqual(Typography.paragraphSpacings, [0, 0.25, 0.5, 0.75, 1, 1.25, 1.5])
    }

    func testJustifiedTextHyphenates() {
        XCTAssertTrue(Typography.hyphenates(align: .justify, hyphenate: false))
        XCTAssertTrue(Typography.hyphenates(align: .left, hyphenate: true))
        XCTAssertFalse(Typography.hyphenates(align: .left, hyphenate: false))
    }

    func testTheLanguageOfAParagraphByItsLetters() {
        XCTAssertEqual(Typography.language(of: "Текст на русском with a few words"), "ru")
        XCTAssertEqual(Typography.language(of: "English text"), "en")
    }

    func testAOneLetterWordStaysWithTheNextWord() {
        let text = Array("идти в лес".utf16)
        XCTAssertFalse(Typography.mayBreak(text, before: 7), "not between в and лес")
        XCTAssertTrue(Typography.mayBreak(text, before: 5), "before в is fine")
        let long = Array("слово кот".utf16)
        XCTAssertTrue(Typography.mayBreak(long, before: 6))
    }

    func testAnEmDashStaysWithTheWordBeforeIt() {
        let text = Array("слово — другое".utf16)
        XCTAssertFalse(Typography.mayBreak(text, before: 6))
        XCTAssertTrue(Typography.mayBreak(text, before: 8))
    }

    func testTheColumnsOfTheSpecification() {
        XCTAssertEqual(ColumnWidth.narrow.characters, 60)
        XCTAssertEqual(ColumnWidth.medium.characters, 72)
        XCTAssertEqual(ColumnWidth.wide.characters, 90)
        XCTAssertNil(ColumnWidth.full.characters)
    }
}

/// E30: the link an attachment gets in the text.
final class AttachmentLinkTests: XCTestCase {
    private func file(_ name: String, _ mime: String) -> Attachment {
        Attachment(id: "a1", noteId: "n", name: name, mime: mime, size: 1, sha256: "", localPath: nil)
    }

    func testWhatShowsInsideIsAnImageLink() {
        XCTAssertEqual(AttachmentLink.text(file("фото.jpg", "image/jpeg")), "![фото.jpg](attachment:a1)")
        XCTAssertEqual(AttachmentLink.text(file("клип.mov", "video/quicktime")), "![клип.mov](attachment:a1)")
        XCTAssertEqual(AttachmentLink.text(file("отчёт.pdf", "application/pdf")), "![отчёт.pdf](attachment:a1)")
        XCTAssertEqual(AttachmentLink.text(file("данные.csv", "text/csv")), "[данные.csv](attachment:a1)")
    }

    func testBracketsInANameStayInTheLink() {
        XCTAssertEqual(AttachmentLink.text(file("a[1].txt", "text/plain")), "[a\\[1\\].txt](attachment:a1)")
    }
}
