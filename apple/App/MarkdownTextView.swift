import AppKit

/// A text view that shows its text as Markdown while it is typed (E10–E16 in docs/specs/editor.md),
/// after the one of Lists.
///
/// The string stays what was typed: the core says which ranges to style, hide
/// and replace, and this view does it with attributes and at the glyph level,
/// so nothing but typing and the edits of the core changes the text.
class MarkdownTextView: NSTextView, NSLayoutManagerDelegate {
    /// The font of plain text; headings and code are derived from it.
    var baseFont = NSFont.systemFont(ofSize: NSFont.systemFontSize) {
        didSet { if baseFont != oldValue { restyle() } }
    }
    /// An edit of the text has been made, with the whole text after it.
    var onEdit: (TextEdit, String) -> Void = { _, _ in }
    /// Opens the address of a link clicked with ⌘ (E13): the web, mail or a note.
    var openLink: (URL) -> Void = { NSWorkspace.shared.open($0) }
    /// The undo manager of the window as SwiftUI gives it to the model: typing and the actions of the
    /// core are in one history.
    var sharedUndoManager: () -> UndoManager? = { nil }
    /// None for the text view itself: it registers its own typing even with `allowsUndo` off, and
    /// those steps, undone in a text this view has changed, go out of its bounds. E25 is `history`.
    override var undoManager: UndoManager? { nil }
    /// E26: whether typing replaces quotes, dashes, dots and arrows.
    var smartTypography = true
    /// The attachments of the note by identifier, for what a link in the text shows (E14, E30).
    var attachment: (String) -> Attachment? = { _ in nil }
    /// A click on an attachment in the text (R31, R32, R35).
    var openAttachment: (String) -> Void = { _ in }
    /// R52: what a link to a note leads to.
    var linkState: (String) -> NoteLinkState = { _ in .live }
    /// R50: the notes for the part of a title typed after `[[`.
    var titlesMatching: (String) -> [Note] = { _ in [] }
    /// Files dropped on the text or pasted into it (R30).
    var addFiles: ([URL]) -> Void = { _ in }
    /// The edit `shouldChangeText` let through and `didChangeText` reports, with what it takes out.
    private var coming: TextEdit?
    private var comingRemoved = ""
    /// E25: what the user typed, for ⌘Z. The text view keeps no history of its own.
    private var history = TypingHistory()
    /// An undo or a redo is being applied: it is not new typing.
    private var replaying = false

    private var concealed = IndexSet()
    /// E14: the first character of each attachment link shown as its attachment, with the size of
    /// the box and the view that lies over it. The rest of the link has no glyphs.
    private var embedded: [Int: NSSize] = [:]
    private var embeds: [(location: Int, id: String)] = []
    private var embedViews: [String: EmbeddedAttachment] = [:]
    private var bullets = IndexSet()
    private var boxes: [(range: NSRange, checked: Bool)] = []
    private var links: [(range: NSRange, url: String)] = []
    private var quotes: [NSRange] = []
    private var rules: [NSRange] = []
    private var codeBlocks: [NSRange] = []
    /// E15: the bars that stand for a tab between two cells, and the tables drawn as a grid.
    private var cellTabs = IndexSet()
    private var grids: [(header: NSRange, width: CGFloat)] = []
    private var styledWidth: CGFloat = 0
    /// The reveal ranges of the spans, with those the selection touches now.
    private var reveals: [(start: Int, end: Int)] = []
    private var touchedReveals: [Int] = []
    private var blocks: [NSRange] = []
    private var focused = false

    /// TextKit 1: hiding works on glyphs, which TextKit 2 does not expose.
    func prepare() {
        layoutManager?.delegate = self
        isRichText = false
    }

    func setText(_ text: String) {
        coming = nil
        history = TypingHistory()
        string = text
        restyle()
    }

    /// E25: the steps of this note leave the undo menu with it.
    func forgetHistory() {
        sharedUndoManager()?.removeAllActions(withTarget: self)
    }

    /// An edit that comes from outside the editor: another device (S40). It keeps the selection on
    /// the same characters and is nothing the user typed.
    func apply(outside edit: TextEdit) {
        guard let storage = textStorage else { return }
        history.outside(edit)
        let selection = selectedRange()
        let start = edit.moved(selection.location)
        let end = edit.moved(selection.location + selection.length)
        storage.replaceCharacters(in: NSRange(location: edit.position, length: edit.deleted), with: edit.inserted)
        super.setSelectedRanges([NSValue(range: NSRange(location: start, length: end - start))], affinity: .downstream, stillSelecting: false)
        restyle()
    }

    /// The edit the text is about to get, as `didChangeText` then reports it.
    override func shouldChangeText(in range: NSRange, replacementString: String?) -> Bool {
        let allowed = super.shouldChangeText(in: range, replacementString: replacementString)
        if allowed, let replacementString {
            coming = TextEdit(position: range.location, deleted: range.length, inserted: replacementString)
            comingRemoved = (string as NSString).substring(with: range)
        }
        return allowed
    }

    // MARK: Styling

    override func didChangeText() {
        super.didChangeText()
        let edit = coming ?? TextEdit(position: 0, deleted: 0, inserted: "")
        coming = nil
        if !replaying, edit.deleted > 0 || !edit.inserted.isEmpty {
            let typed = TypedEdit(position: edit.position, removed: comingRemoved, inserted: edit.inserted)
            if history.record(typed, at: Date()) { registerUndo() }
        }
        onEdit(edit, string)
        restyle()
    }

    // MARK: Undo of typing (E25)

    private func registerUndo() {
        sharedUndoManager()?.registerUndo(withTarget: self) { $0.undoTyping() }
        sharedUndoManager()?.setActionName(L("Typing"))
    }

    private func undoTyping() {
        guard let step = history.popUndo() else { return }
        replay(step.reversed().map(\.inverse))
        sharedUndoManager()?.registerUndo(withTarget: self) { $0.redoTyping() }
    }

    private func redoTyping() {
        guard let step = history.popRedo() else { return }
        replay(step.map(\.forward))
        registerUndo()
    }

    private func replay(_ edits: [TextEdit]) {
        replaying = true
        defer { replaying = false }
        for edit in edits {
            replace(NSRange(location: edit.position, length: edit.deleted), with: edit.inserted)
            setSelectedRange(NSRange(location: edit.position + edit.insertedUnits, length: 0))
        }
        scrollRangeToVisible(selectedRange())
    }

    // MARK: Edits of the core (E21–E23, E26)

    /// Applies an edit of the core as one step of undo, with the selection it asks for.
    private func apply(_ edit: MarkdownEdit) {
        history.close()
        replace(NSRange(location: Int(edit.start), length: Int(edit.end - edit.start)), with: edit.text)
        let (first, last) = (Int(min(edit.anchor, edit.cursor)), Int(max(edit.anchor, edit.cursor)))
        setSelectedRange(NSRange(location: first, length: last - first))
        history.close()
        scrollRangeToVisible(selectedRange())
    }

    /// E20, E21: a format on the selection or the word at the cursor.
    func format(_ format: MarkdownFormat) {
        guard isEditable, !hasMarkedText() else { return }
        let selection = selectedRange()
        let edit = markdownFormat(text: string, start: UInt32(selection.location), end: UInt32(NSMaxRange(selection)), format: format)
        edit.map(apply)
    }

    /// E22: Return in a list item or a quote.
    override func insertNewline(_ sender: Any?) {
        let selection = selectedRange()
        if selection.length == 0, !hasMarkedText(), let edit = markdownNewline(text: string, cursor: UInt32(selection.location)) {
            apply(edit)
        } else {
            super.insertNewline(sender)
        }
    }

    /// E22: Tab and ⇧Tab change the level of a list item; elsewhere Tab is a tab.
    override func insertTab(_ sender: Any?) {
        if !indent(outdent: false) { super.insertTab(sender) }
    }

    override func insertBacktab(_ sender: Any?) {
        _ = indent(outdent: true)
    }

    private func indent(outdent: Bool) -> Bool {
        let selection = selectedRange()
        guard let edit = markdownIndent(text: string, start: UInt32(selection.location), end: UInt32(NSMaxRange(selection)), outdent: outdent) else { return false }
        apply(edit)
        return true
    }

    /// E23: rich text comes in as Markdown of the dialect, and an address over a selection as a link.
    override func paste(_ sender: Any?) {
        if !paste(from: .general) { super.paste(sender) }
    }

    /// Whether the core had something to do with what is on the pasteboard.
    func paste(from pasteboard: NSPasteboard) -> Bool {
        let files = IncomingFiles.pasted(pasteboard)
        if !files.isEmpty {
            addFiles(files)
            files.forEach(IncomingFiles.discard)
            return true
        }
        let selection = selectedRange()
        guard let edit = markdownPaste(
            text: string, start: UInt32(selection.location), end: UInt32(NSMaxRange(selection)),
            plain: pasteboard.string(forType: .string), html: pasteboard.string(forType: .html)) else { return false }
        apply(edit)
        return true
    }

    /// E26: what typing replaces, as a step of its own, so that undo right after it brings back
    /// what was typed.
    override func insertText(_ text: Any, replacementRange: NSRange) {
        super.insertText(text, replacementRange: replacementRange)
        guard !hasMarkedText(), !replaying else { return }
        let cursor = selectedRange()
        if smartTypography, cursor.length == 0, let edit = markdownTyped(text: string, cursor: UInt32(cursor.location)) { apply(edit) }
        updateLinkPicker()
    }

    // MARK: Links to notes (R50)

    private var picker: LinkPicker?

    /// The place of `[[` before the cursor on its line, with what was typed after it.
    private var linkQuery: (start: Int, text: String)? {
        let cursor = selectedRange()
        guard cursor.length == 0 else { return nil }
        let text = string as NSString
        let line = text.lineRange(for: NSRange(location: cursor.location, length: 0))
        let before = text.substring(with: NSRange(location: line.location, length: cursor.location - line.location)) as NSString
        let open = before.range(of: "[[", options: .backwards)
        guard open.location != NSNotFound else { return nil }
        let typed = before.substring(from: NSMaxRange(open))
        return typed.contains("]") ? nil : (line.location + open.location, typed)
    }

    /// E24: `[[` opens the picker of notes, and what follows narrows it.
    private func updateLinkPicker() {
        guard let query = linkQuery else { return closeLinkPicker() }
        let notes = titlesMatching(query.text)
        if notes.isEmpty { return closeLinkPicker() }
        if picker == nil { picker = LinkPicker { [weak self] note in self?.finishLink(note) } }
        let glyphs = layoutManager?.glyphRange(forCharacterRange: NSRange(location: max(query.start, 0), length: 1), actualCharacterRange: nil) ?? NSRange()
        let rect = (textContainer.flatMap { layoutManager?.boundingRect(forGlyphRange: glyphs, in: $0) } ?? .zero)
            .offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
        picker?.show(notes, near: rect, of: self)
    }

    private func closeLinkPicker() {
        picker?.close()
        picker = nil
    }

    /// The choice takes the place of `[[…`; the text of the link is the title of the moment (E31).
    private func finishLink(_ note: Note) {
        guard let query = linkQuery else { return closeLinkPicker() }
        closeLinkPicker()
        let title = note.shownTitle.replacingOccurrences(of: "[", with: "\\[").replacingOccurrences(of: "]", with: "\\]")
        let link = "[\(title)](note:\(note.id))"
        let range = NSRange(location: query.start, length: selectedRange().location - query.start)
        history.close()
        replace(range, with: link)
        setSelectedRange(NSRange(location: query.start + (link as NSString).length, length: 0))
        history.close()
    }

    /// While the picker is open the arrows, Return and Esc are its own.
    override func doCommand(by selector: Selector) {
        if let picker {
            switch selector {
            case #selector(moveUp(_:)): picker.move(-1); return
            case #selector(moveDown(_:)): picker.move(1); return
            case #selector(insertNewline(_:)), #selector(insertTab(_:)): picker.choose(); return
            case #selector(cancelOperation(_:)): closeLinkPicker(); return
            default: break
            }
        }
        super.doCommand(by: selector)
        if selector == #selector(deleteBackward(_:)), picker != nil { updateLinkPicker() }
    }

    /// R53: the selection as UTF-16 offsets, for a new note made from it.
    var selectedOffsets: (start: UInt32, end: UInt32)? {
        let selection = selectedRange()
        return selection.length > 0 ? (UInt32(selection.location), UInt32(NSMaxRange(selection))) : nil
    }

    /// E20: the formats in the context menu.
    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = super.menu(for: event) ?? NSMenu()
        let formats = NSMenuItem(title: L("Format"), action: nil, keyEquivalent: "")
        let sub = NSMenu()
        for (title, format) in FormatItem.all {
            let item = NSMenuItem(title: title, action: #selector(chooseFormat(_:)), keyEquivalent: "")
            item.representedObject = FormatItem(format: format)
            item.target = self
            sub.addItem(item)
        }
        formats.submenu = sub
        menu.insertItem(formats, at: 0)
        if selectedRange().length > 0 {
            let item = NSMenuItem(title: L("New Note from Selection"), action: #selector(newNoteFromSelection), keyEquivalent: "")
            item.target = self
            menu.insertItem(item, at: 1)
        }
        menu.insertItem(.separator(), at: selectedRange().length > 0 ? 2 : 1)
        return menu
    }

    /// R53: asks the app to make the note; it is the core that moves the text.
    var makeNoteFromSelection: () -> Void = {}

    @objc private func newNoteFromSelection() { makeNoteFromSelection() }

    @objc private func chooseFormat(_ item: NSMenuItem) {
        (item.representedObject as? FormatItem).map { format($0.format) }
    }

    override func setSelectedRanges(_ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool) {
        super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
        if !stillSelecting, focused, touched() != touchedReveals { restyle() }
    }

    override func becomeFirstResponder() -> Bool {
        let became = super.becomeFirstResponder()
        if became { focused = true; restyle() }
        return became
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned { focused = false; restyle() }
        return resigned
    }

    /// The reveal ranges the selection touches, by number. None without the keyboard.
    private func touched() -> [Int] {
        guard focused else { return [] }
        let selection = selectedRange()
        return reveals.indices.filter { Reveal.touches(selection: selection, start: reveals[$0].start, end: reveals[$0].end) }
    }

    /// E47: code is 0.9 of the text size.
    private func code(_ font: NSFont) -> NSFont {
        NSFont.monospacedSystemFont(ofSize: font.pointSize * 0.9, weight: .regular)
    }

    /// E3: the colour of a token of code.
    private static func color(_ kind: CodeClass) -> NSColor {
        switch kind {
        case .keyword: .systemPink
        case .operator: .secondaryLabelColor
        case .string: .systemRed
        case .number, .constant: .systemPurple
        case .comment: .systemGray
        case .type: .systemTeal
        case .function: .systemBlue
        case .tag: .systemBlue
        case .attribute: .systemOrange
        case .property: .systemIndigo
        case .inserted: .systemGreen
        case .deleted: .systemRed
        case .heading: .systemBlue
        case .markup: .secondaryLabelColor
        }
    }

    private func derived(_ font: NSFont, _ traits: NSFontDescriptor.SymbolicTraits, size: CGFloat? = nil) -> NSFont {
        let descriptor = font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(traits))
        return NSFont(descriptor: descriptor, size: size ?? font.pointSize) ?? font
    }

    private func restyle() {
        // Attributes set under text that is still being composed would end the composition.
        guard let storage = textStorage, let layout = layoutManager, !hasMarkedText() else { return }
        let info = markdownLayout(text: string)
        let length = storage.length
        let full = NSRange(location: 0, length: length)
        let text = storage.string as NSString
        blocks = info.blocks.map { NSRange(location: Int($0.start), length: Int($0.end - $0.start)) }
        reveals = info.spans.map { (Int($0.revealStart), Int($0.revealEnd)) }
        touchedReveals = touched()
        let selection = selectedRange()
        let shows = { (start: Int, end: Int) in self.focused && Reveal.touches(selection: selection, start: start, end: end) }
        let dim = NSColor.tertiaryLabelColor
        let plain: [NSAttributedString.Key: Any] = [.font: baseFont, .foregroundColor: NSColor.labelColor]

        var hidden = IndexSet(), bullets = IndexSet(), tabs = IndexSet()
        var boxes2: [Int: NSSize] = [:]
        var newEmbeds: [(location: Int, id: String)] = []
        boxes = []; links = []; quotes = []; rules = []; codeBlocks = []; grids = []
        styledWidth = textContainer?.size.width ?? 0

        // E15: the tables to draw as a grid, each with the lines of its rows. A table
        // stays as typed while the cursor is in it, and where it does not start its
        // line: in a list item or a quote.
        var tables: [(table: MarkdownTable, lines: [NSRange], rule: NSRange)] = []
        for table in info.tables {
            let range = NSRange(location: Int(table.start), length: Int(table.end - table.start))
            guard NSMaxRange(range) <= length, !shows(range.location, NSMaxRange(range)),
                  text.lineRange(for: NSRange(location: range.location, length: 0)).location == range.location else { continue }
            var lines: [NSRange] = []
            text.enumerateSubstrings(in: range, options: [.byLines, .substringNotRequired]) { _, line, _, _ in lines.append(line) }
            guard lines.count == table.rows.count + 1 else { continue }
            tables.append((table, [lines[0]] + lines[2...], lines[1]))
        }
        let inGrid = { (range: NSRange) in tables.contains { $0.lines.contains { NSIntersectionRange($0, range).length > 0 } || $0.rule == range } }

        func retrait(_ range: NSRange, _ change: (NSFont) -> NSFont) {
            storage.enumerateAttribute(.font, in: range) { value, run, _ in
                if let font = value as? NSFont { storage.addAttribute(.font, value: change(font), range: run) }
            }
        }
        func indent(_ range: NSRange, first: CGFloat, rest: CGFloat) {
            let paragraph = text.paragraphRange(for: range)
            let current = storage.attribute(.paragraphStyle, at: paragraph.location, effectiveRange: nil) as? NSParagraphStyle
            let style = (current?.mutableCopy() as? NSMutableParagraphStyle) ?? NSMutableParagraphStyle()
            style.firstLineHeadIndent += first
            style.headIndent += rest
            storage.addAttribute(.paragraphStyle, value: style, range: paragraph)
        }
        /// Markup that is a whole line, a code fence, leaves no empty line behind when hidden.
        func collapse(_ range: NSRange) {
            let line = text.lineRange(for: range)
            let content = text.substring(with: line).trimmingCharacters(in: .newlines)
            guard content.utf16.count == range.length, NSMaxRange(line) < length || line.location > 0 else { return }
            let style = NSMutableParagraphStyle()
            style.maximumLineHeight = 0.01
            storage.addAttributes([.font: NSFont.systemFont(ofSize: 0.01), .paragraphStyle: style], range: line)
        }
        let boxSide = ceil(baseFont.pointSize * 1.05)

        storage.beginEditing()
        storage.setAttributes(plain, range: full)
        for span in info.spans {
            let range = NSRange(location: Int(span.start), length: Int(span.end - span.start))
            guard NSMaxRange(range) <= length, Int(span.block) < blocks.count else { continue }
            let shown = shows(Int(span.revealStart), Int(span.revealEnd))
            switch span.kind {
            case .heading(let level):
                // E2, E47: levels 4 to 6 look like level 3.
                let scale: [CGFloat] = [1.6, 1.35, 1.15, 1.15, 1.15, 1.15]
                let size = baseFont.pointSize * scale[max(0, min(5, Int(level) - 1))]
                storage.addAttributes([.font: derived(baseFont, .bold, size: size), .foregroundColor: NSColor.labelColor], range: range)
            case .strong:
                retrait(range) { self.derived($0, .bold) }
            case .emphasis:
                retrait(range) { self.derived($0, .italic) }
            case .strikethrough:
                storage.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: range)
            case .highlight:
                storage.addAttribute(.backgroundColor, value: NSColor.systemYellow.withAlphaComponent(0.35), range: range)
            case .code:
                retrait(range) { self.code($0) }
                storage.addAttribute(.backgroundColor, value: NSColor.quaternaryLabelColor, range: range)
            case .codeBlock:
                retrait(range) { self.code($0) }
                codeBlocks.append(range)
            case .token(let kind):
                storage.addAttribute(.foregroundColor, value: Self.color(kind), range: range)
            case .tableRow:
                if !inGrid(range) { retrait(range) { self.code($0) } }
            case .quote:
                quotes.append(range)
                indent(range, first: 12, rest: 12)
            case .link(let url):
                links.append((range, url))
                storage.addAttributes([.foregroundColor: NSColor.linkColor, .underlineStyle: NSUnderlineStyle.single.rawValue], range: range)
            case .noteLink(let id):
                links.append((range, "note:\(id)"))
                // R52: a link to a note in the Trash or deleted for good shows as broken.
                let broken = linkState(id) != .live
                storage.addAttributes([
                    .foregroundColor: broken ? NSColor.systemRed : NSColor.linkColor,
                    .underlineStyle: (broken ? NSUnderlineStyle.patternDot.union(.single) : .single).rawValue,
                ], range: range)
            case .attachmentLink(let id, _):
                // E14: the line of the link is source while the cursor is on it.
                if shown {
                    storage.addAttribute(.foregroundColor, value: NSColor.secondaryLabelColor, range: range)
                } else {
                    let view = embedView(id)
                    let size = view.size(forWidth: columnWidth)
                    boxes2[range.location] = size
                    newEmbeds.append((range.location, id))
                    hidden.insert(integersIn: (range.location + 1)..<NSMaxRange(range))
                    let paragraph = text.paragraphRange(for: range)
                    let current = storage.attribute(.paragraphStyle, at: paragraph.location, effectiveRange: nil) as? NSParagraphStyle
                    let style = (current?.mutableCopy() as? NSMutableParagraphStyle) ?? NSMutableParagraphStyle()
                    style.minimumLineHeight = max(style.minimumLineHeight, size.height + 4)
                    storage.addAttribute(.paragraphStyle, value: style, range: paragraph)
                }
            case .listMarker(let ordered):
                if shown, !ordered { storage.addAttribute(.foregroundColor, value: dim, range: range) }
                if !ordered, !shown, range.length == 1 { bullets.insert(range.location) }
                // Wrapped lines of an item start under its text, not under its marker. Not
                // so behind a hidden quote sign: a line that begins with hidden glyphs is
                // set at the indent of the wrapped ones, and the marker would move.
                let line = text.lineRange(for: range)
                if !hidden.contains(line.location) {
                    let lead = text.substring(with: NSRange(location: line.location, length: NSMaxRange(range) - line.location)) + " "
                    indent(range, first: 0, rest: (lead as NSString).size(withAttributes: [.font: baseFont]).width)
                }
            case .quoteMarker:
                if shown { storage.addAttribute(.foregroundColor, value: dim, range: range) } else { hidden.insert(integersIn: range.location..<NSMaxRange(range)) }
            case .checkbox(let checked):
                // The brackets come back only when the cursor is right at them.
                let touched = focused && selection.location <= NSMaxRange(range) && range.location <= NSMaxRange(selection)
                if touched || range.length != 3 {
                    storage.addAttribute(.foregroundColor, value: dim, range: range)
                } else {
                    let mark = NSRange(location: range.location + 1, length: 1)
                    let width = (text.substring(with: mark) as NSString).size(withAttributes: [.font: baseFont]).width
                    hidden.insert(range.location)
                    hidden.insert(range.location + 2)
                    storage.addAttributes([.font: baseFont, .foregroundColor: NSColor.clear, .kern: boxSide - width], range: mark)
                    boxes.append((range, checked))
                }
            case .rule:
                if shown {
                    storage.addAttribute(.foregroundColor, value: dim, range: range)
                } else {
                    rules.append(range)
                    storage.addAttribute(.foregroundColor, value: NSColor.clear, range: range)
                }
            case .markup:
                if shown {
                    storage.addAttribute(.foregroundColor, value: dim, range: range)
                } else {
                    hidden.insert(integersIn: range.location..<NSMaxRange(range))
                    collapse(range)
                }
            }
        }
        for (table, lines, rule) in tables {
            let cells = table.rows.map { $0.cells.map { NSRange(location: Int($0.start), length: Int($0.end - $0.start)) } }
            let columns = table.columns.count
            // What the parser made of a row has to lie in its line, in order; otherwise the table stays as typed.
            let sound = columns > 0 && zip(cells, lines).allSatisfy { row, line in
                row.count == columns && row.first!.location >= line.location && NSMaxRange(row.last!) <= NSMaxRange(line)
                    && zip(row, row.dropFirst()).allSatisfy { NSMaxRange($0) <= $1.location }
            }
            for (row, header) in zip(cells, table.rows.map(\.header)) where sound && header {
                row.forEach { cell in retrait(cell) { self.derived($0, .bold) } }
                storage.addAttribute(.foregroundColor, value: NSColor.labelColor, range: NSUnionRange(row.first!, row.last!))
            }
            /// The width of a cell as it is seen: without the markup hidden in it.
            func width(_ cell: NSRange) -> CGFloat {
                let seen = NSMutableAttributedString(attributedString: storage.attributedSubstring(from: cell))
                for index in (0..<cell.length).reversed() where hidden.contains(cell.location + index) {
                    seen.deleteCharacters(in: NSRange(location: index, length: 1))
                }
                return ceil(seen.size().width)
            }
            let widths = sound ? cells.map { $0.map(width) } : []
            let column = (0..<columns).map { index in widths.map { $0[index] }.max() ?? 0 }
            let gap = ceil(baseFont.pointSize * 1.4)
            let total = column.reduce(0, +) + gap * CGFloat(max(0, columns - 1))
            // A grid is not wrapped: a table wider than the note stays as typed.
            guard sound, styledWidth <= 0 || total <= styledWidth else {
                for line in lines + [rule] { retrait(line) { self.code($0) } }
                continue
            }
            var stops: [NSTextTab] = [], left: CGFloat = 0
            for index in 0..<columns {
                if index > 0 {
                    switch table.columns[index] {
                    case .right: stops.append(NSTextTab(textAlignment: .right, location: left + column[index]))
                    case .center: stops.append(NSTextTab(textAlignment: .center, location: left + column[index] / 2))
                    default: stops.append(NSTextTab(textAlignment: .left, location: left))
                    }
                }
                left += column[index] + gap
            }
            for (number, (row, line)) in zip(cells, lines).enumerated() {
                hidden.insert(integersIn: line.location..<row[0].location)
                hidden.insert(integersIn: NSMaxRange(row[columns - 1])..<NSMaxRange(line))
                for (cell, next) in zip(row, row.dropFirst()) {
                    let between = NSRange(location: NSMaxRange(cell), length: next.location - NSMaxRange(cell))
                    let bar = text.range(of: "|", range: between)
                    for index in between.location..<NSMaxRange(between) {
                        if index == bar.location { tabs.insert(index) } else { hidden.insert(index) }
                    }
                }
                let style = NSMutableParagraphStyle()
                style.tabStops = stops
                style.lineBreakMode = .byClipping
                style.paragraphSpacing = 3
                // The first cell has no tab before it: it is moved by the indent of its line.
                let spare = column[0] - widths[number][0]
                style.firstLineHeadIndent = table.columns[0] == .right ? spare : table.columns[0] == .center ? spare / 2 : 0
                storage.addAttribute(.paragraphStyle, value: style, range: text.paragraphRange(for: line))
            }
            // The line of dashes takes no room.
            hidden.insert(integersIn: rule.location..<NSMaxRange(rule))
            let gone = NSMutableParagraphStyle()
            gone.maximumLineHeight = 0.01
            storage.addAttributes([.font: NSFont.systemFont(ofSize: 0.01), .paragraphStyle: gone], range: text.lineRange(for: rule))
            grids.append((lines[0], total))
        }
        storage.endEditing()
        typingAttributes = plain

        embeds = newEmbeds
        if hidden != concealed || bullets != self.bullets || tabs != cellTabs || boxes2 != embedded {
            concealed = hidden
            self.bullets = bullets
            cellTabs = tabs
            embedded = boxes2
            layout.invalidateGlyphs(forCharacterRange: full, changeInLength: 0, actualCharacterRange: nil)
            layout.invalidateLayout(forCharacterRange: full, actualCharacterRange: nil)
        }
        placeAttachments()
        invalidateIntrinsicContentSize()
        needsDisplay = true
    }

    /// The width a box may take: the column without the padding of the line.
    private var columnWidth: CGFloat {
        (textContainer?.size.width ?? 400) - 2 * (textContainer?.lineFragmentPadding ?? 0)
    }

    private func embedView(_ id: String) -> EmbeddedAttachment {
        if let known = embedViews[id] { return known }
        let view = EmbeddedAttachment(id: id, file: attachment(id))
        view.onClick = { [weak self] in self?.openAttachment(id) }
        embedViews[id] = view
        addSubview(view)
        return view
    }

    /// Puts each attachment over its box, and takes away those no longer shown.
    private func placeAttachments() {
        guard let layout = layoutManager, let container = textContainer else { return }
        layout.ensureLayout(for: container)
        let shown = Set(embeds.map(\.id))
        for (id, view) in embedViews where !shown.contains(id) {
            view.removeFromSuperview()
            embedViews[id] = nil
        }
        for embed in embeds {
            let glyphs = layout.glyphRange(forCharacterRange: NSRange(location: embed.location, length: 1), actualCharacterRange: nil)
            let box = layout.boundingRect(forGlyphRange: glyphs, in: container)
            let size = embedded[embed.location] ?? box.size
            embedViews[embed.id]?.frame = NSRect(
                x: box.minX + textContainerOrigin.x, y: box.minY + textContainerOrigin.y + 2, width: size.width, height: size.height)
        }
    }

    /// The content of an attachment reached this device, or a link changed what it leads to.
    func reloadAttachments() {
        for (id, view) in embedViews where view.kind != EmbeddedAttachment(id: id, file: attachment(id)).kind {
            view.removeFromSuperview()
            embedViews[id] = nil
        }
        restyle()
    }

    /// E30: a link to the file at the cursor.
    func insert(link: String) {
        replace(selectedRange(), with: link)
        setSelectedRange(NSRange(location: selectedRange().location + (link as NSString).length, length: 0))
    }

    /// R37: scrolls to the place of a piece of the text and puts the cursor there.
    func reveal(_ piece: String) {
        let found = (string as NSString).range(of: piece)
        guard found.location != NSNotFound else { return }
        setSelectedRange(NSRange(location: found.location, length: 0))
        scrollRangeToVisible(found)
    }

    // MARK: Files

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if urls.isEmpty || !isEditable { return super.performDragOperation(sender) }
        addFiles(urls)
        return true
    }

    // MARK: Glyphs

    func layoutManager(
        _ layoutManager: NSLayoutManager, shouldGenerateGlyphs glyphs: UnsafePointer<CGGlyph>,
        properties: UnsafePointer<NSLayoutManager.GlyphProperty>, characterIndexes: UnsafePointer<Int>,
        font: NSFont, forGlyphRange range: NSRange
    ) -> Int {
        var newGlyphs = Array(UnsafeBufferPointer(start: glyphs, count: range.length))
        var newProperties = Array(UnsafeBufferPointer(start: properties, count: range.length))
        var changed = false
        for i in 0..<range.length {
            let character = characterIndexes[i]
            if concealed.contains(character) {
                newProperties[i] = .null
                changed = true
            } else if embedded[character] != nil || cellTabs.contains(character) {
                newProperties[i] = .controlCharacter
                changed = true
            } else if bullets.contains(character) {
                var bullet: [UniChar] = [0x2022]
                var glyph: [CGGlyph] = [0]
                if CTFontGetGlyphsForCharacters(font as CTFont, &bullet, &glyph, 1) {
                    newGlyphs[i] = glyph[0]
                    changed = true
                }
            }
        }
        guard changed else { return 0 }
        layoutManager.setGlyphs(newGlyphs, properties: newProperties, characterIndexes: characterIndexes, font: font, forGlyphRange: range)
        return range.length
    }

    /// E15: the bar between two cells of a grid acts as a tab to the next column.
    func layoutManager(
        _ layoutManager: NSLayoutManager, shouldUse action: NSLayoutManager.ControlCharacterAction, forControlCharacterAt charIndex: Int
    ) -> NSLayoutManager.ControlCharacterAction {
        if embedded[charIndex] != nil { return .whitespace }
        return cellTabs.contains(charIndex) ? .horizontalTab : action
    }

    /// E14: the box of an attachment: its width. The height of its line comes from the paragraph.
    func layoutManager(
        _ layoutManager: NSLayoutManager, boundingBoxForControlGlyphAt glyphIndex: Int, for textContainer: NSTextContainer,
        proposedLineFragment proposedRect: NSRect, glyphPosition: NSPoint, characterIndex: Int
    ) -> NSRect {
        let size = embedded[characterIndex] ?? .zero
        return NSRect(x: glyphPosition.x, y: 0, width: size.width, height: size.height)
    }

    /// Whether a table fits depends on the width, so the note is styled again when that changes.
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if let width = textContainer?.size.width, width != styledWidth, !embeds.isEmpty || !markdownLayout(text: string).tables.isEmpty { restyle() }
    }

    // MARK: Drawing

    private func rect(of range: NSRange) -> NSRect? {
        guard let layout = layoutManager, let container = textContainer, NSMaxRange(range) <= (textStorage?.length ?? 0) else { return nil }
        // Hidden glyphs have no place of their own and would stretch the rectangle.
        var range = range
        while range.length > 1, concealed.contains(range.location) { range = NSRange(location: range.location + 1, length: range.length - 1) }
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        return layout.boundingRect(forGlyphRange: glyphs, in: container).offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
    }

    private func boxRect(_ range: NSRange) -> NSRect? {
        guard let mark = rect(of: NSRange(location: range.location + 1, length: 1)) else { return nil }
        let side = ceil(baseFont.pointSize * 1.05)
        return NSRect(x: mark.minX, y: mark.midY - side / 2, width: side, height: side)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.tertiaryLabelColor.setFill()
        for quote in quotes {
            guard let rect = rect(of: quote) else { continue }
            NSRect(x: textContainerOrigin.x, y: rect.minY, width: 3, height: rect.height).fill()
        }
        for rule in rules {
            guard let rect = rect(of: rule) else { continue }
            NSRect(x: textContainerOrigin.x, y: rect.midY, width: bounds.width - textContainerOrigin.x * 2, height: 1).fill()
        }
        for grid in grids {
            guard let rect = rect(of: grid.header) else { continue }
            NSRect(x: textContainerOrigin.x, y: rect.maxY + 1, width: grid.width, height: 1).fill()
        }
        NSColor.quaternaryLabelColor.setFill()
        for block in codeBlocks {
            guard let rect = rect(of: block) else { continue }
            let width = bounds.width - textContainerOrigin.x * 2
            NSBezierPath(roundedRect: NSRect(x: textContainerOrigin.x, y: rect.minY, width: width, height: rect.height), xRadius: 4, yRadius: 4).fill()
        }
        super.draw(dirtyRect)
        for box in boxes {
            guard let rect = boxRect(box.range) else { continue }
            let configuration = NSImage.SymbolConfiguration(pointSize: rect.height * 0.85, weight: .regular)
                .applying(.init(paletteColors: box.checked ? [.white, .controlAccentColor] : [.secondaryLabelColor]))
            let name = box.checked ? "checkmark.square.fill" : "square"
            let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
            image?.draw(in: rect.insetBy(dx: 1, dy: 1))
        }
    }

    // MARK: Clicks and Return

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        // E13: a click with ⌘ opens a link; a plain click on it places the cursor.
        if event.modifierFlags.contains(.command), let url = link(at: point) {
            openLink(url)
            return
        }
        if !click(at: point) { super.mouseDown(with: event) }
    }

    /// Toggles the checkbox under the point (E12).
    @discardableResult
    func click(at point: NSPoint) -> Bool {
        guard let box = box(at: point), isEditable else { return false }
        toggle(box.range, checked: box.checked)
        return true
    }

    private func box(at point: NSPoint) -> (range: NSRange, checked: Bool)? {
        boxes.first { boxRect($0.range)?.insetBy(dx: -2, dy: -2).contains(point) == true }
    }

    /// The address of the link whose text is under the point.
    private func link(at point: NSPoint) -> URL? {
        guard let layout = layoutManager, let container = textContainer else { return nil }
        let inContainer = NSPoint(x: point.x - textContainerOrigin.x, y: point.y - textContainerOrigin.y)
        var fraction: CGFloat = 0
        let glyph = layout.glyphIndex(for: inContainer, in: container, fractionOfDistanceThroughGlyph: &fraction)
        guard layout.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container).contains(inContainer) else { return nil }
        let character = layout.characterIndexForGlyph(at: glyph)
        return links.first { NSLocationInRange(character, $0.range) }.flatMap { URL(string: $0.url) }
    }

    private func toggle(_ range: NSRange, checked: Bool) {
        history.close()
        replace(NSRange(location: range.location + 1, length: 1), with: checked ? " " : "x")
        history.close()
    }

    /// An edit the way typing makes one, so that Undo takes it back.
    func replace(_ range: NSRange, with text: String) {
        guard shouldChangeText(in: range, replacementString: text) else { return }
        textStorage?.replaceCharacters(in: range, with: text)
        didChangeText()
    }

    #if DEBUG
    /// What a person would see, for the debug script: the text without the hidden characters.
    var debugSeen: String {
        String(decoding: Array(string.utf16).enumerated().compactMap { index, unit -> UInt16? in
            if concealed.contains(index) { return nil }
            if bullets.contains(index) { return 0x2022 }
            return unit
        }, as: UTF16.self)
    }

    /// The notes the picker of links offers.
    var debugPicker: [String]? { picker?.debugTitles }

    /// The cursor goes to this offset, as a click there would put it.
    func debugCaret(_ offset: Int) {
        setSelectedRange(NSRange(location: min(offset, (string as NSString).length), length: 0))
    }
    #endif
}

/// A format as an object of a menu item.
final class FormatItem: NSObject {
    let format: MarkdownFormat
    init(format: MarkdownFormat) { self.format = format }

    /// E20: the formats in the order of the menus.
    static let all: [(String, MarkdownFormat)] = [
        (L("Bold"), .bold), (L("Italic"), .italic), (L("Strikethrough"), .strikethrough), (L("Code"), .code),
        (L("Highlight"), .highlight), (L("Link"), .link), (L("Heading 1"), .heading(level: 1)), (L("Heading 2"), .heading(level: 2)),
        (L("Heading 3"), .heading(level: 3)), (L("Plain Paragraph"), .paragraph), (L("Bulleted List"), .bulletList),
        (L("Numbered List"), .numberedList), (L("Task List"), .taskList), (L("Quote"), .quote),
    ]
}

/// R50: the list of notes that `[[` opens at the cursor.
final class LinkPicker: NSObject {
    private let popover = NSPopover()
    private let table = NSTableView()
    private var notes: [Note] = []
    private let chosen: (Note) -> Void

    init(chosen: @escaping (Note) -> Void) {
        self.chosen = chosen
        super.init()
        table.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("title")))
        table.headerView = nil
        table.dataSource = self
        table.delegate = self
        table.action = #selector(clicked)
        table.target = self
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 280, height: 180))
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        let controller = NSViewController()
        controller.view = scroll
        popover.contentViewController = controller
        popover.behavior = .semitransient
    }

    func show(_ notes: [Note], near rect: NSRect, of view: NSView) {
        self.notes = notes
        table.reloadData()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        if !popover.isShown { popover.show(relativeTo: rect, of: view, preferredEdge: .maxY) }
    }

    func move(_ step: Int) {
        let row = max(0, min(notes.count - 1, table.selectedRow + step))
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    func choose() {
        if notes.indices.contains(table.selectedRow) { chosen(notes[table.selectedRow]) }
    }

    func close() { popover.close() }

    @objc private func clicked() { choose() }

    var debugTitles: [String] { notes.map(\.shownTitle) }
}

extension LinkPicker: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { notes.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        NSTextField(labelWithString: notes[row].shownTitle)
    }
}
