import Foundation

struct Account: Codable, Identifiable, Hashable {
    let id: Int64
    var name: String
    var email: String
    var createdAt: Date
}

struct FolderCount: Codable, Hashable {
    let folder: String
    let total: Int
    let unread: Int
}

struct MessageSummary: Codable, Identifiable, Hashable {
    let id: Int64
    let accountId: Int64
    var folder: String
    let fromName: String
    let fromAddr: String
    let toAddrs: String
    let subject: String
    let snippet: String
    var isRead: Bool
    var isStarred: Bool
    let createdAt: Date
}

struct Message: Codable, Identifiable, Hashable {
    let id: Int64
    let accountId: Int64
    var folder: String
    let fromName: String
    let fromAddr: String
    let toAddrs: String
    let ccAddrs: String
    let subject: String
    let body: String
    var isRead: Bool
    var isStarred: Bool
    let inReplyTo: Int64?
    let createdAt: Date
}

struct Compose: Codable, Hashable {
    var accountId: Int64
    var to: String = ""
    var cc: String = ""
    var subject: String = ""
    var body: String = ""
    var inReplyTo: Int64? = nil
    var draftId: Int64? = nil
}

struct SendResult: Codable {
    let message: Message
    let deliveredLocal: Int
    let undelivered: [String]
}

struct MessagePatch: Codable {
    var isRead: Bool? = nil
    var isStarred: Bool? = nil
    var folder: String? = nil
}

struct NewAccount: Codable {
    let name: String
    let email: String
}

/// Mailbox folders in sidebar order, matching the server's fixed order.
enum Folder: String, CaseIterable, Identifiable {
    case inbox, starred, sent, drafts, archive, trash

    var id: String { rawValue }

    var title: String {
        switch self {
        case .inbox: "Inbox"
        case .starred: "Starred"
        case .sent: "Sent"
        case .drafts: "Drafts"
        case .archive: "Archive"
        case .trash: "Trash"
        }
    }

    var symbol: String {
        switch self {
        case .inbox: "tray"
        case .starred: "star"
        case .sent: "paperplane"
        case .drafts: "doc.text"
        case .archive: "archivebox"
        case .trash: "trash"
        }
    }

    /// Folders where the unread count is meaningful to show as a badge.
    var showsUnreadBadge: Bool {
        self == .inbox || self == .starred || self == .archive
    }
}

extension MessageSummary {
    var senderDisplay: String { fromName.isEmpty ? fromAddr : fromName }
}

extension Message {
    var senderDisplay: String { fromName.isEmpty ? fromAddr : fromName }
}

enum Initials {
    static func of(_ name: String) -> String {
        let parts = name
            .split(whereSeparator: { $0 == " " || $0 == "@" || $0 == "." })
            .prefix(2)
        let letters = parts.compactMap(\.first).map { String($0).uppercased() }
        return letters.isEmpty ? "?" : letters.joined()
    }
}
