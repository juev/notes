import Foundation

/// Tells when an entry appears in a folder or leaves it.
final class FolderWatcher {
    private var source: DispatchSourceFileSystemObject?
    private var watched: URL?

    /// Starts watching `folder`, or stops when it is `nil`. Asking for the
    /// folder already watched changes nothing; a folder that cannot be opened
    /// is not watched, and the next call tries again.
    func watch(_ folder: URL?, onChange: @escaping () -> Void) {
        if folder == watched, source != nil || folder == nil { return }
        source?.cancel()
        source = nil
        watched = folder
        guard let folder else { return }
        let descriptor = open(folder.path, O_EVTONLY)
        guard descriptor >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: descriptor, eventMask: [.write, .delete, .rename], queue: .main)
        source.setEventHandler(handler: onChange)
        source.setCancelHandler { close(descriptor) }
        source.resume()
        self.source = source
    }
}
