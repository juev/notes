import AVKit
import AppKit
import PDFKit

/// An attachment in its place in the text (E14): an image, a player, the first page of a PDF
/// (R31), or a chip (R32). It lies over the box the text view leaves for it.
final class EmbeddedAttachment: NSView {
    enum Kind: Equatable {
        case image, video, audio, pdf, chip, pending, broken
    }

    let kind: Kind
    let attachmentId: String
    var onClick: () -> Void = {}
    private var natural = NSSize(width: 320, height: 200)

    init(id: String, file: Attachment?) {
        attachmentId = id
        kind = Self.kind(of: file)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.masksToBounds = true
        build(file)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a file") }

    private static func kind(of file: Attachment?) -> Kind {
        guard let file else { return .broken }
        guard file.localPath != nil else { return .pending }
        if file.mime.hasPrefix("image/") { return .image }
        if file.mime.hasPrefix("video/") { return .video }
        if file.mime.hasPrefix("audio/") { return .audio }
        if file.mime == "application/pdf" { return .pdf }
        return .chip
    }

    /// E30: the size of the box for a column of this width. An image takes the width of the column at
    /// most (R31).
    func size(forWidth width: CGFloat) -> NSSize {
        let width = max(width, 80)
        switch kind {
        case .image, .pdf:
            let scale = min(1, width / max(natural.width, 1))
            return NSSize(width: ceil(natural.width * scale), height: ceil(natural.height * scale))
        case .video: return NSSize(width: min(width, 480), height: ceil(min(width, 480) * 9 / 16))
        case .audio: return NSSize(width: min(width, 360), height: 44)
        case .chip, .pending, .broken: return NSSize(width: min(width, natural.width), height: 32)
        }
    }

    private func build(_ file: Attachment?) {
        switch kind {
        case .image:
            guard let path = file?.localPath, let image = NSImage(contentsOfFile: path) else { return chip(file) }
            natural = image.size
            let view = NSImageView(image: image)
            view.imageScaling = .scaleProportionallyUpOrDown
            fill(with: view)
        case .video, .audio:
            guard let file, let url = AttachmentFiles.named(file) else { return chip(file) }
            let view = AVPlayerView()
            view.player = AVPlayer(url: url)
            view.controlsStyle = .inline
            fill(with: view)
        case .pdf:
            guard let path = file?.localPath, let document = PDFDocument(url: URL(fileURLWithPath: path)), let page = document.page(at: 0) else { return chip(file) }
            let bounds = page.bounds(for: .mediaBox)
            natural = NSSize(width: min(320, bounds.width), height: min(320, bounds.width) * bounds.height / max(bounds.width, 1))
            let image = NSImageView(image: page.thumbnail(of: natural, for: .mediaBox))
            image.imageScaling = .scaleProportionallyUpOrDown
            fill(with: image)
            let pages = NSTextField(labelWithString: document.pageCount == 1 ? L("1 page") : L("%d pages", document.pageCount))
            pages.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            pages.drawsBackground = true
            pages.backgroundColor = .windowBackgroundColor
            pages.translatesAutoresizingMaskIntoConstraints = false
            addSubview(pages)
            NSLayoutConstraint.activate([pages.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6), pages.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -6)])
        case .chip, .pending, .broken:
            chip(file)
        }
    }

    private func fill(with view: NSView) {
        view.translatesAutoresizingMaskIntoConstraints = false
        addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: leadingAnchor), view.trailingAnchor.constraint(equalTo: trailingAnchor),
            view.topAnchor.constraint(equalTo: topAnchor), view.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    /// R32: an icon, the name and the size; R35: a file on its way says so; E30: a link to an
    /// attachment of another note shows as broken.
    private func chip(_ file: Attachment?) {
        layer?.backgroundColor = NSColor.quaternaryLabelColor.cgColor
        let icon = NSImageView(image: NSImage(systemSymbolName: kind == .broken ? "exclamationmark.triangle" : kind == .pending ? "arrow.down.circle" : "doc", accessibilityDescription: nil) ?? NSImage())
        let text: String
        switch kind {
        case .broken: text = L("Attachment not found")
        case .pending: text = L("%@ — waiting for its content", file?.name ?? "")
        default: text = "\(file?.name ?? "") — \(ByteCountFormatter.string(fromByteCount: Int64(file?.size ?? 0), countStyle: .file))"
        }
        let label = NSTextField(labelWithString: text)
        label.lineBreakMode = .byTruncatingMiddle
        let stack = NSStackView(views: [icon, label])
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8), stack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -8),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        natural = NSSize(width: ceil(label.intrinsicContentSize.width) + 48, height: 32)
    }

    override func mouseDown(with event: NSEvent) {
        if kind == .video || kind == .audio { super.mouseDown(with: event) } else { onClick() }
    }
}
