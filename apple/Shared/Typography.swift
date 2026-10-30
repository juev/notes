import Foundation

/// E44: the width of the column, a setting of the device.
enum ColumnWidth: String, CaseIterable {
    case narrow, medium, wide, full

    /// About how many characters the column holds; nil for the full width of the window.
    var characters: Int? {
        switch self {
        case .narrow: 60
        case .medium: 72
        case .wide: 90
        case .full: nil
        }
    }
}

/// E40–E48: the settings of typography and what they come to.
enum Typography {
    /// E41: the size on this device, 12 to 28 points; nothing saved is the body size of the platform.
    static let sizes = 12...28
    static func size(saved: Int?, system: Double) -> Double {
        guard let saved else { return system }
        return Double(min(max(saved, sizes.lowerBound), sizes.upperBound))
    }

    /// E40: the fonts on offer, as the settings name them.
    static let textFonts = ["system", "Literata", "Inter", "iA Writer Quattro"]
    static let codeFonts = ["system", "JetBrains Mono"]
    /// The family of a font in the files built in.
    static func family(_ name: String) -> String? {
        switch name {
        case "Literata": "Literata"
        case "Inter": "Inter"
        case "iA Writer Quattro": "iA Writer Quattro V"
        case "JetBrains Mono": "JetBrains Mono"
        default: nil
        }
    }

    /// E42, E43: the steps of the sliders.
    static let lineHeights = stride(from: 1.2, through: 2.0, by: 0.1).map { ($0 * 10).rounded() / 10 }
    static let paragraphSpacings = stride(from: 0.0, through: 1.5, by: 0.25).map { $0 }

    /// E45: justified text turns hyphenation on; with left alignment it is a switch of its own.
    static func hyphenates(align: TextAlign, hyphenate: Bool) -> Bool {
        align == .justify || hyphenate
    }

    /// E46: the language of a paragraph: Russian for Cyrillic text, English for Latin.
    static func language(of paragraph: String) -> String {
        let cyrillic = paragraph.unicodeScalars.filter { (0x0400...0x04FF).contains($0.value) }.count
        let latin = paragraph.unicodeScalars.filter { $0.isASCII && $0.properties.isAlphabetic }.count
        return cyrillic > latin ? "ru" : "en"
    }

    /// E48: whether a line may break before the character at `index` of `text`: a one-letter Russian
    /// word stays with the word after it, and an em dash with the word before it.
    static func mayBreak(_ text: [UInt16], before index: Int) -> Bool {
        guard index > 0, index < text.count else { return true }
        let dash: UInt16 = 0x2014
        if text[index] == dash { return false }
        // What stands before the break, without the spaces: one letter after a space or the line start?
        var end = index
        while end > 0, text[end - 1] == 0x20 || text[end - 1] == 0xA0 { end -= 1 }
        guard end > 0, end < index else { return true }
        let letters: Set<UInt16> = Set("авикосуяАВИКОСУЯ".utf16)
        guard letters.contains(text[end - 1]) else { return true }
        return end >= 2 && !(text[end - 2] == 0x20 || text[end - 2] == 0xA0 || text[end - 2] == 0x0A)
    }
}
