import AppKit
import Foundation

/// State for one compose window. Codable/Hashable so it can drive a
/// `WindowGroup(for:)` scene.
struct ComposeState: Codable, Hashable, Identifiable {
    var id = UUID()
    var compose: Compose
    var draftId: Int64?
}

struct Banner: Identifiable, Equatable {
    enum Kind { case info, warning, error }
    let id = UUID()
    let kind: Kind
    let text: String
}

@MainActor
final class MailStore: ObservableObject {
    enum Connection: Equatable {
        case connecting
        case ready
        case failed(String)
    }

    @Published private(set) var connection: Connection = .connecting
    @Published private(set) var accounts: [Account] = []
    @Published private(set) var folderCounts: [Folder: FolderCount] = [:]
    @Published private(set) var messages: [MessageSummary] = []
    @Published private(set) var selectedMessage: Message?
    @Published private(set) var isLoadingMessages = false
    @Published var banner: Banner?

    @Published var selectedAccountId: Int64? {
        didSet {
            guard selectedAccountId != oldValue else { return }
            selectedMessageId = nil
            messages = []
            folderCounts = [:]
            Task { await reload() }
        }
    }

    @Published var selectedFolder: Folder = .inbox {
        didSet {
            guard selectedFolder != oldValue else { return }
            selectedMessageId = nil
            messages = []
            Task { await loadMessages() }
        }
    }

    @Published var selectedMessageId: Int64? {
        didSet {
            guard selectedMessageId != oldValue else { return }
            Task { await loadSelectedMessage() }
        }
    }

    @Published var searchText = "" {
        didSet {
            guard searchText != oldValue else { return }
            searchTask?.cancel()
            searchTask = Task {
                try? await Task.sleep(for: .milliseconds(250))
                guard !Task.isCancelled else { return }
                await loadMessages()
            }
        }
    }

    private let api = APIClient()
    private var pollTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var bannerTask: Task<Void, Never>?

    var selectedAccount: Account? {
        accounts.first { $0.id == selectedAccountId }
    }

    // MARK: - Lifecycle

    func start() async {
        connection = .connecting
        do {
            try await ServerManager.shared.ensureRunning()
            accounts = try await api.accounts()
            connection = .ready
            if selectedAccountId == nil || selectedAccount == nil {
                selectedAccountId = accounts.first?.id
            } else {
                await reload()
            }
            startPolling()
        } catch {
            connection = .failed(error.localizedDescription)
        }
    }

    private func startPolling() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard let self, !Task.isCancelled else { return }
                await self.refresh(silently: true)
            }
        }
    }

    /// Refreshes counts and the current list. When silent, transient errors are ignored.
    func refresh(silently: Bool = false) async {
        guard connection == .ready else { return }
        do {
            let latest = try await api.accounts()
            if latest != accounts { accounts = latest }
            if selectedAccount == nil { selectedAccountId = accounts.first?.id }
            try await loadCounts()
            try await fetchMessages()
        } catch {
            if !silently { show(error) }
        }
    }

    func reload() async {
        do {
            try await loadCounts()
            try await fetchMessages()
        } catch {
            show(error)
        }
    }

    // MARK: - Loading

    private func loadCounts() async throws {
        guard let accountId = selectedAccountId else { return }
        let counts = try await api.folderCounts(accountId: accountId)
        guard accountId == selectedAccountId else { return }
        var map: [Folder: FolderCount] = [:]
        for count in counts {
            if let folder = Folder(rawValue: count.folder) { map[folder] = count }
        }
        folderCounts = map
        let unread = map[.inbox]?.unread ?? 0
        NSApp.dockTile.badgeLabel = unread > 0 ? String(unread) : nil
    }

    func loadMessages() async {
        do {
            try await fetchMessages()
        } catch {
            show(error)
        }
    }

    private func fetchMessages() async throws {
        guard let accountId = selectedAccountId else { return }
        let folder = selectedFolder
        let query = searchText
        isLoadingMessages = messages.isEmpty
        defer { isLoadingMessages = false }
        let result = try await api.messages(accountId: accountId, folder: folder, query: query)
        // Discard results for a selection that has since changed.
        guard accountId == selectedAccountId, folder == selectedFolder, query == searchText else { return }
        if result != messages { messages = result }
        if let id = selectedMessageId, !result.contains(where: { $0.id == id }) {
            selectedMessageId = nil
        }
    }

    private func loadSelectedMessage() async {
        guard let id = selectedMessageId else {
            selectedMessage = nil
            return
        }
        do {
            let message = try await api.message(id)
            guard id == selectedMessageId else { return }
            selectedMessage = message
            if !message.isRead {
                await setRead(id, true)
            }
        } catch {
            show(error)
        }
    }

    // MARK: - Message actions

    private func patchLocal(_ id: Int64, _ apply: (inout MessageSummary) -> Void) {
        if let i = messages.firstIndex(where: { $0.id == id }) {
            apply(&messages[i])
        }
    }

    func summary(_ id: Int64) -> MessageSummary? {
        messages.first { $0.id == id }
    }

    func setRead(_ id: Int64, _ read: Bool) async {
        patchLocal(id) { $0.isRead = read }
        if selectedMessage?.id == id { selectedMessage?.isRead = read }
        await perform { try await self.api.updateMessage(id, MessagePatch(isRead: read)) }
    }

    func toggleStar(_ id: Int64) async {
        let starred = !(summary(id)?.isStarred ?? selectedMessage?.isStarred ?? false)
        patchLocal(id) { $0.isStarred = starred }
        if selectedMessage?.id == id { selectedMessage?.isStarred = starred }
        await perform { try await self.api.updateMessage(id, MessagePatch(isStarred: starred)) }
    }

    func move(_ id: Int64, to folder: Folder) async {
        let wasSelected = selectedMessageId == id
        let next = wasSelected ? neighbor(of: id) : nil
        removeLocal(id)
        await perform { try await self.api.updateMessage(id, MessagePatch(folder: folder.rawValue)) }
        if wasSelected { selectedMessageId = next }
        flash(.info, folder == .archive ? "Archived" : "Moved to \(folder.title)")
    }

    func delete(_ id: Int64) async {
        let wasSelected = selectedMessageId == id
        let next = wasSelected ? neighbor(of: id) : nil
        removeLocal(id)
        var permanent = false
        await perform { permanent = try await self.api.deleteMessage(id) }
        if wasSelected { selectedMessageId = next }
        flash(.info, permanent ? "Deleted permanently" : "Moved to Trash")
    }

    func markAllRead(in folder: Folder) async {
        guard let accountId = selectedAccountId else { return }
        if folder == selectedFolder {
            for i in messages.indices { messages[i].isRead = true }
        }
        await perform { try await self.api.markAllRead(accountId: accountId, folder: folder) }
    }

    func emptyTrash() async {
        guard let accountId = selectedAccountId else { return }
        var n = 0
        await perform { n = try await self.api.emptyTrash(accountId: accountId) }
        if selectedFolder == .trash {
            messages = []
            selectedMessageId = nil
        }
        flash(.info, n == 1 ? "Deleted 1 message" : "Deleted \(n) messages")
    }

    private func removeLocal(_ id: Int64) {
        messages.removeAll { $0.id == id }
        if selectedMessage?.id == id { selectedMessage = nil }
    }

    private func neighbor(of id: Int64) -> Int64? {
        guard let i = messages.firstIndex(where: { $0.id == id }) else { return nil }
        if i + 1 < messages.count { return messages[i + 1].id }
        return i > 0 ? messages[i - 1].id : nil
    }

    /// Runs a mutation, then refreshes counts and list to reconcile with the server.
    private func perform(_ op: @escaping () async throws -> Void) async {
        do {
            try await op()
        } catch {
            show(error)
        }
        await refresh(silently: true)
    }

    // MARK: - Accounts

    func createAccount(name: String, email: String) async throws {
        let account = try await api.createAccount(name: name, email: email)
        accounts.append(account)
        selectedAccountId = account.id
        flash(.info, "Created \(account.email)")
    }

    // MARK: - Compose

    func newCompose() -> ComposeState? {
        guard let accountId = selectedAccountId else { return nil }
        return ComposeState(compose: Compose(accountId: accountId))
    }

    func replyCompose(all: Bool) -> ComposeState? {
        guard let m = selectedMessage, let me = selectedAccount else { return nil }
        let own = me.email.lowercased()
        // Replying to something I sent goes back to the original recipients.
        let original = m.fromAddr.lowercased() == own ? splitAddresses(m.toAddrs) : [m.fromAddr]
        var to = original
        var cc: [String] = []
        if all {
            to += splitAddresses(m.toAddrs)
            cc = splitAddresses(m.ccAddrs)
        }
        to = unique(to.filter { $0.lowercased() != own })
        if to.isEmpty { to = original } // a note to self
        cc = unique(cc.filter { $0.lowercased() != own && !to.contains($0) })
        let date = m.createdAt.formatted(date: .abbreviated, time: .shortened)
        let quoted = m.body.split(separator: "\n", omittingEmptySubsequences: false)
            .map { "> \($0)" }
            .joined(separator: "\n")
        return ComposeState(compose: Compose(
            accountId: me.id,
            to: to.joined(separator: ", "),
            cc: cc.joined(separator: ", "),
            subject: prefixed("Re:", m.subject),
            body: "\n\nOn \(date), \(m.senderDisplay) <\(m.fromAddr)> wrote:\n\(quoted)",
            inReplyTo: m.id
        ))
    }

    func forwardCompose() -> ComposeState? {
        guard let m = selectedMessage, let me = selectedAccount else { return nil }
        let date = m.createdAt.formatted(date: .abbreviated, time: .shortened)
        var header = "---------- Forwarded message ----------\n"
        header += "From: \(m.senderDisplay) <\(m.fromAddr)>\nDate: \(date)\nSubject: \(m.subject)\nTo: \(m.toAddrs)\n"
        if !m.ccAddrs.isEmpty { header += "Cc: \(m.ccAddrs)\n" }
        return ComposeState(compose: Compose(
            accountId: me.id,
            subject: prefixed("Fwd:", m.subject),
            body: "\n\n\(header)\n\(m.body)",
            inReplyTo: m.id
        ))
    }

    func editDraftCompose() -> ComposeState? {
        guard let m = selectedMessage, m.folder == Folder.drafts.rawValue else { return nil }
        return ComposeState(
            compose: Compose(
                accountId: m.accountId, to: m.toAddrs, cc: m.ccAddrs, subject: m.subject,
                body: m.body, inReplyTo: m.inReplyTo, draftId: m.id
            ),
            draftId: m.id
        )
    }

    /// Sends a message. Throws so the compose window can show the error inline.
    func send(_ compose: Compose) async throws {
        let result = try await api.sendMessage(compose)
        if result.undelivered.isEmpty {
            flash(.info, "Message sent")
        } else {
            flash(
                .warning,
                "Sent, but not delivered to \(result.undelivered.joined(separator: ", ")) — only local accounts receive mail."
            )
        }
        await refresh(silently: true)
    }

    func saveDraft(_ compose: Compose, id: Int64?) async throws -> Int64 {
        let draft = try await api.saveDraft(compose, id: id)
        await refresh(silently: true)
        return draft.id
    }

    // MARK: - Banners

    func show(_ error: Error) {
        flash(.error, error.localizedDescription)
    }

    func flash(_ kind: Banner.Kind, _ text: String) {
        let banner = Banner(kind: kind, text: text)
        self.banner = banner
        bannerTask?.cancel()
        bannerTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(kind == .info ? 3 : 6))
            guard !Task.isCancelled, self?.banner == banner else { return }
            self?.banner = nil
        }
    }
}

// MARK: - Helpers

func splitAddresses(_ list: String) -> [String] {
    list.split(whereSeparator: { $0 == "," || $0 == ";" })
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .filter { !$0.isEmpty }
}

private func unique(_ items: [String]) -> [String] {
    var seen = Set<String>()
    return items.filter { seen.insert($0.lowercased()).inserted }
}

private func prefixed(_ prefix: String, _ subject: String) -> String {
    subject.lowercased().hasPrefix(prefix.lowercased()) ? subject : "\(prefix) \(subject)"
}
