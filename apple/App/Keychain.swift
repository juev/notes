import Foundation
import Security

/// The secrets of sync live in the login keychain, the way mail and calendar accounts keep theirs
/// (R82). The core never sees them on disk: it gets them in memory after the store is opened.
///
/// All of them are in one item, read once in a run: a build signed ad hoc has another signature
/// every time, so after an update the keychain asks again for every item the app reads, and with
/// one item it asks once. As in Lists.
enum Keychain {
    /// A check that has a data folder of its own has an item of its own as well.
    private static let suffix: String = {
        #if DEBUG
        if Storage.debugDirectory != nil { return ".debug" }
        #endif
        return ""
    }()
    private static let service = "org.evsyukov.notes" + suffix
    private static let item = "secrets"

    private static let lock = NSLock()
    /// The secrets by account, once the item was read. An empty value is a secret that was removed.
    private static var secrets: [String: String]?

    private static func query() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: item,
        ]
    }

    /// The secrets of the one item. Nil when the keychain did not let the item be read: nothing is
    /// written over it then, and it is asked for again the next time.
    private static func all() -> [String: String]? {
        if let secrets { return secrets }
        var request = query()
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        switch SecItemCopyMatching(request as CFDictionary, &found) {
        case errSecSuccess:
            secrets = (found as? Data).flatMap { try? JSONDecoder().decode([String: String].self, from: $0) } ?? [:]
        case errSecItemNotFound:
            secrets = [:]
        default:
            return nil
        }
        return secrets
    }

    private static func store(_ all: [String: String]) throws {
        let data = try JSONEncoder().encode(all)
        var status = SecItemUpdate(query() as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var new = query()
            new[kSecValueData as String] = data
            new[kSecAttrLabel as String] = "Notes"
            status = SecItemAdd(new as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw failure(status) }
        secrets = all
    }

    private static func failure(_ status: OSStatus) -> NSError {
        NSError(domain: NSOSStatusErrorDomain, code: Int(status), userInfo: [
            NSLocalizedDescriptionKey: SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)",
        ])
    }

    /// One secret per server and user, so switching back finds the old password.
    static func account(url: String, user: String) -> String { "\(user)@\(url)" }

    /// The access token of an ntfy server that requires sign-in, one secret per server.
    static func pushAccount(server: String) -> String { "ntfy:\(server)" }

    /// The passphrase of an encrypted storage, one secret per storage.
    static func passphraseAccount(_ config: SyncConfig) -> String? {
        switch config {
        case .webDav(let url, let user): return "passphrase:\(user)@\(url)"
        case .folder(let path): return "passphrase:\(path)"
        case .off: return nil
        }
    }

    static func load(account: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        guard let known = all()?[account], !known.isEmpty else { return nil }
        return known
    }

    static func delete(account: String) {
        lock.lock()
        defer { lock.unlock() }
        guard var all = all() else { return }
        all[account] = ""
        try? store(all)
    }

    static func save(_ secret: String, account: String) throws {
        lock.lock()
        defer { lock.unlock() }
        guard var all = all() else { throw failure(errSecAuthFailed) }
        all[account] = secret
        try store(all)
    }
}
