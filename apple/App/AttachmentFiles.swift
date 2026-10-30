import AppKit
import UniformTypeIdentifiers

/// Attachments as files the system can show, as in Lists. The content lives under the hash of its
/// bytes, without a name or an extension, so Quick Look and the default app cannot tell what it is.
/// They get a copy under the name the file was attached with. A copy and not a link: an app that
/// saves over the file it opened must not change the content behind the hash.
enum AttachmentFiles {
    private static let root = FileManager.default.temporaryDirectory.appendingPathComponent("notes-attachments", isDirectory: true)

    /// The content of an attachment under its own name, or nil while it is not on this device.
    static func named(_ file: Attachment) -> URL? {
        guard let path = file.localPath else { return nil }
        let manager = FileManager.default
        // Two attachments may share a name; their content tells them apart.
        let dir = root.appendingPathComponent(file.sha256, isDirectory: true)
        let target = dir.appendingPathComponent(file.name.replacingOccurrences(of: "/", with: "-"))
        if manager.fileExists(atPath: target.path) { return target }
        do {
            try manager.createDirectory(at: dir, withIntermediateDirectories: true)
            try manager.copyItem(at: URL(fileURLWithPath: path), to: target)
            return target
        } catch {
            return nil
        }
    }

    /// Removes the named copies; they are made again when asked for.
    static func clear() {
        try? FileManager.default.removeItem(at: root)
    }

    /// E30: what shows inside the note and so is written as an image link.
    static func isInline(_ mime: String) -> Bool {
        mime.hasPrefix("image/") || mime.hasPrefix("video/") || mime.hasPrefix("audio/") || mime == "application/pdf"
    }

    /// E30: the link to an attachment in the text.
    static func link(_ file: Attachment) -> String {
        let name = file.name.replacingOccurrences(of: "[", with: "\\[").replacingOccurrences(of: "]", with: "\\]")
        return "\(isInline(file.mime) ? "!" : "")[\(name)](attachment:\(file.id))"
    }
}

/// Files on their way into a note: picked in the open panel, dropped or pasted, as in Lists.
@MainActor
enum IncomingFiles {
    private static let pastedPrefix = "notes-pasted-"

    /// What ⌘V attaches. Files copied in Finder come as they are; a bare image (a screenshot, "Copy
    /// Image") is written out as a PNG first. Empty when the pasteboard holds text: an image that
    /// only accompanies text, as a copied table does, is not what is being pasted.
    static func pasted(_ pasteboard: NSPasteboard) -> [URL] {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !urls.isEmpty { return urls }
        guard pasteboard.string(forType: .string) == nil, let png = png(pasteboard) else { return [] }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(pastedPrefix + UUID().uuidString, isDirectory: true)
        let stamp = DateFormatter()
        stamp.locale = Locale(identifier: "en_US_POSIX")
        stamp.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        let file = dir.appendingPathComponent("Image \(stamp.string(from: Date())).png")
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try png.write(to: file)
            return [file]
        } catch {
            return []
        }
    }

    static func pick() -> [URL] {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        return panel.runModal() == .OK ? panel.urls : []
    }

    /// Removes the temporary copy of a pasted image; other files are left alone.
    static func discard(_ url: URL) {
        let dir = url.deletingLastPathComponent()
        if dir.lastPathComponent.hasPrefix(pastedPrefix) { try? FileManager.default.removeItem(at: dir) }
    }

    private static func png(_ pasteboard: NSPasteboard) -> Data? {
        if let data = pasteboard.data(forType: .png) { return data }
        guard let tiff = pasteboard.data(forType: .tiff) else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }
}
