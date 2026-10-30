# Notes: editor, Markdown and typography

Status: the part in the core is implemented in `core/` (stage 2) and checked by tests named after the requirements: the parsing of the dialect (E1–E5, E10–E16, E30, E31, E51) in `core/tests/markdown.rs`, highlighting (E3) in `core/tests/highlight.rs`, and the edits E21–E23 and E26 in `core/tests/editing.rs`; Return in a list (E22) was there from Lists. Painting, undo of typing (E25), typography (E40–E48) and spelling (E50, E52) belong to the apps; the CSS of export to HTML follows E40–E48.

The macOS app (`apple/`, stage 3) implements the rest for that platform. Checked by Swift tests in `apple/Tests/` (E11, E25 positions, E41–E45, E48) and by debug scenarios with pictures of the window: E1–E3, E11–E16, E21–E23, E25, E26, E30, E45, E46. Not looked at: E48 on a real line end, E50 and E51 underlines, the shortcuts of E20 pressed on a keyboard (the scenarios call the same functions).

Android and the web are not built. The parsing and painting scheme is taken from Lists, where it is implemented for task notes (`core/src/markdown.rs`, `MarkdownTextView.swift`, `MarkdownField.kt` there). Sources: the request of the author of the project and the discussion of 2026-10-29; the use of the Super editor in the author's Standard Notes backup ([architecture.md](../architecture.md)).

Size of highlighting (E3): the core built for Android (`cargo ndk -t arm64-v8a build --release --lib`) grew from 6 144 000 to 8 547 648 bytes when highlighting was added, on 2026-10-30. The web server is measured when it is built.

## Purpose and scope

This document describes what a note may contain, how it is shown while it is edited, how formatting is applied, and the global settings of typography. Where the attachments and links live in the data is in [sync.md](sync.md) and [product.md](product.md).

The rule from the request: within a note the user changes only the kind of text — bold, italic, code, headings, lists and the like. The font, its size, the spacing and the alignment are set once for all notes.

## Requirements

### The dialect

- E1. A note is CommonMark with the GitHub extensions: tables, task list items (`- [ ]`, `- [x]`), strikethrough (`~~text~~`) and autolinks. Added on top: highlight (`==text==`).
- E2. Headings of levels 1 to 6 are recognised; levels 1, 2 and 3 have their own size, and levels 4 to 6 look like level 3 (E47).
- E3. Fenced code blocks carry a language after the fence; the core highlights at least `bash`, `sh`, `shell`, `zsh`, `javascript`, `typescript`, `json`, `yaml`, `toml`, `markdown`, `python`, `go`, `rust`, `swift`, `kotlin`, `sql`, `html`, `css`, `diff` and `dockerfile`, and also any other language its syntaxes know; `text`/`plain` and a language they do not know are shown without highlighting. Highlighting runs in the core with `syntect` and its pure-Rust regex engine (`fancy-regex`), so the core needs no C library on any platform; it returns ranges with a token class, like the rest of the parsing (E10). The syntaxes built in are those of bat as the crate `two-face` packs them (213 at version 0.5.2). A set of only the languages above was planned and is not possible: syntect cannot cut a built set down, and building one from the sources of the syntaxes was judged not worth it. The licenses of the syntaxes ask for a notice; the core gives its text (`code_highlight_acknowledgements`) and the apps show it with the other notices of the app. syntect needs `bincode` 1.3.3, which RustSec marks unmaintained; it decodes only the syntaxes built into the binary, so the advisory is ignored in `deny.toml` with that reason. The growth of the core on Android and of the web server is recorded in the status of this document. Decisions of the author of the project of 2026-10-29 and 2026-10-30.
- E4. Raw HTML in a note is shown as text and is never interpreted. A link with a scheme other than `http`, `https`, `mailto`, `attachment` and `note` is shown but does not open.
- E5. The core departs from CommonMark where Lists does: a lone `-` under a line of text starts a list item instead of turning the line above into a heading.

### Showing the formatting

- E10. The core parses the text and returns ranges in UTF-16 units: what to style, what to hide, and what to replace with a bullet, a rule, a checkbox, an attachment or a table. The platforms only paint; they do not parse Markdown. On the web the ranges come with the text, and the page builds text nodes from them, not markup.
- E11. Markup characters of inline formatting (`**`, `_`, `` ` ``, `~~`, `==`) are hidden. They are shown, dimmed, while the cursor is inside the formatted span or right next to it, so that the user sees what will be deleted.
- E12. The `#` of a heading and the `>` of a quote are hidden and shown, dimmed, while the cursor is on that line. A list marker is replaced with a bullet or kept as a number; a task list item shows a checkbox, and a tap on it toggles `[ ]` and `[x]` in the text. A thematic break is drawn as a rule.
- E13. A link shows its text styled as a link and hides the address; the address is shown while the cursor is inside the link. A click with ⌘ on macOS, a tap on Android and the web open it.
- E14. An attachment link (E30) is replaced by the attachment as R31 and R32 in [product.md](product.md) describe. The line of the link is shown as source while the cursor is on it.
- E15. A table is drawn as a grid while the cursor is outside it, and shown as aligned source while the cursor is inside.
- E16. A code block is drawn in the monospace font with a background and its highlighting; its fences are hidden while the cursor is outside the block.

### Editing

- E20. Formatting is applied from the toolbar, from the context menu and by shortcuts on macOS and the web: ⌘B bold, ⌘I italic, ⇧⌘X strikethrough, ⌘E code, ⇧⌘H highlight, ⌘K link, ⌘1 to ⌘3 headings, ⌘0 plain paragraph, ⇧⌘8 bulleted list, ⇧⌘7 numbered list, ⇧⌘9 task list, ⇧⌘. quote. On Android the same actions are on a bar above the keyboard. Typing the markers by hand gives the same result on every platform, since the text is Markdown.
- E21. Applying a format inserts its markers around the selection or the word at the cursor; applying it again where it is already set removes the markers. A line format replaces the line format the line already has.
- E22. Return in a list item continues the list with the next marker; Return in an empty item ends the list. Tab and ⇧Tab change the level of a list item. The core decides the edit (`markdown_newline` in Lists), and the platform applies it as ordinary input.
- E23. Pasting rich text (HTML from a browser or a word processor) converts it in the core to Markdown of this dialect; pasting as plain text (⌥⇧⌘V on macOS, the menu elsewhere) inserts the text as it is. Pasting a URL over a selection makes a link with the selection as its text. Pasting an image or a file creates an attachment (R30 in [product.md](product.md)).
- E24. Typing `[[` opens the note picker of R50 in [product.md](product.md). Typing `/` at the start of an empty line is ordinary text: there is no block menu.
- E25. Undo in the editor (⌘Z, ⇧⌘Z on macOS; the system undo on Android; the browser's on the web) undoes the user's own typing on this device, in steps separated by pauses and by formatting actions. Edits that arrived from other devices are not undone. The undo history lives for as long as the note stays open.
- E26. With `smart_typography` on (the default), typing replaces: straight double quotes with «» in Russian text and “” in English text, the language taken from the letter before the quote; a hyphen between spaces with an em dash with a non-breaking space before it; three dots with an ellipsis; `->` with →. Nothing is replaced inside code and inside link addresses. Undo right after a replacement brings back what was typed. The text keeps the replaced characters.

### Attachments and links in the text

- E30. An attachment is written in the text as `![<name>](attachment:<id>)` when it is an image, a video, an audio file or a PDF, and as `[<name>](attachment:<id>)` otherwise. The identifier is that of the attachment entity ([sync.md](sync.md)). A link to an attachment of another note shows as broken; copying text with attachment links into another note copies the attachments with it.
- E31. A link to a note is written as `[<text>](note:<id>)`. The text is the title at the moment of insertion and is not rewritten when the note is renamed, so a rename does not change the text of other notes; the link still opens the note by its identifier. Export turns such links into `[[<title>]]` with the current title (R60 in [product.md](product.md)).

### Typography

All settings in this section apply to every note, to the editor and to export to HTML and PDF. They sync across devices (S12 in [sync.md](sync.md)), except the font size and the column width, which are set on each device.

- E40. **Fonts.** The text font is one of: System, Literata (serif), Inter (sans-serif), iA Writer Quattro. The code font is JetBrains Mono or the system monospace font. The fonts other than the system ones are bundled with the apps and served by the web server, so a note looks the same everywhere; each is under the SIL Open Font License, which is checked when the font is added.
- E41. **Size**, on the device: from 12 to 28 points, in steps of 1; the default is the body size of the platform.
- E42. **Line height:** from 1.2 to 2.0 of the font size, in steps of 0.1; the default is 1.5.
- E43. **Paragraph spacing:** from 0 to 1.5 of the line, in steps of 0.25; the default is 0.5.
- E44. **Column width**, on the device: narrow (about 60 characters), medium (about 72, the default), wide (about 90), or the full width of the window. The text is centred in the window.
- E45. **Alignment:** left (the default) or justified. Justified text turns hyphenation on, since without it the gaps between words grow; with left alignment hyphenation is a separate switch, off by default.
- E46. **Hyphenation** uses the rules of the language of the paragraph: Russian for Cyrillic text, English for Latin. On macOS this is `hyphenationFactor` of the paragraph style, on Android `hyphenationFrequency`, on the web CSS `hyphens: auto` with `lang` on the paragraph. Headings, code and tables are not hyphenated.
- E47. **Fixed proportions** that the user does not set: heading 1 is 1.6 of the text size, heading 2 1.35, heading 3 1.15, all in the text font with a heavier weight; code is 0.9 of the text size; quotes are indented with a line on the left; the space before a heading is twice the paragraph spacing.
- E48. **Line breaking** keeps a one-letter Russian word (а, в, и, к, о, с, у, я) on the same line as the word after it, and an em dash on the same line as the word before it. This is done when the text is laid out; the stored text does not change.

### Spelling

- E50. Spelling is checked by the system: `NSSpellChecker` on macOS, the browser on the web, the system spell checker service on Android. The switch `spellcheck` (on by default) and the language `spell_language` (automatic, or a chosen one) apply on all devices.
- E51. Misspelled words are underlined only in text: not in code, link addresses, attachment links or URLs. The ranges come from the core (E10).
- E52. A word added to the dictionary goes to the dictionary of the system on that device and does not sync.

## Verification scenarios

The parsing rules are covered by tests in `core/tests/markdown.rs` named after their identifier; painting is checked in the apps by hand, and the specification records which checks were made.

- E1, E10: each construct of the dialect gives the expected ranges, with offsets in UTF-16 units, including text with emoji and Cyrillic.
- E11: `a **b** c` with the cursor outside → the asterisks are hidden; with the cursor after `b` → they are shown.
- E21: ⌘B on a word → `**word**`; ⌘B again → `word`.
- E22: Return after `- item` → `- `; Return on an empty `- ` → the marker is removed.
- E23: pasting a table from a browser → a GFM table; pasting a link over the word "here" → `[here](https://…)`.
- E25: A types, B's edit arrives, A presses undo → A's typing is undone, B's edit stays.
- E26: `"слово"` → «слово»; `"word"` → “word”; `a - b` → `a — b`; inside a code span nothing changes; undo after the replacement → the straight quote is back.
- E31: a link to a note, the note renamed → the link text is unchanged and the link opens the note; the export has `[[<new title>]]`.
- E45, E46: justified alignment on → long Russian and English words are hyphenated in paragraphs, not in headings or code; the same note exported to PDF breaks the same way as in the editor on macOS (checked by eye).
- E48: a line ending in `в` → `в` moves to the next line with its word.
- E51: a misspelled word inside a code span → not underlined.

## To check

- E50: whether the text field on Android, painted by our own editor, gets the system spell checker. If it does not, the editor asks the spell checker service of the system itself and paints the underlines from its answers. Checked on an emulator before the Android editor is built.
