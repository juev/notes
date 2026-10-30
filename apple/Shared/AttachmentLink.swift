import Foundation

/// E30: an attachment in the text: an image link for what shows inside the note, a link otherwise.
enum AttachmentLink {
    static func isInline(_ mime: String) -> Bool {
        mime.hasPrefix("image/") || mime.hasPrefix("video/") || mime.hasPrefix("audio/") || mime == "application/pdf"
    }

    static func text(_ file: Attachment) -> String {
        let name = file.name.replacingOccurrences(of: "[", with: "\\[").replacingOccurrences(of: "]", with: "\\]")
        return "\(isInline(file.mime) ? "!" : "")[\(name)](attachment:\(file.id))"
    }
}
