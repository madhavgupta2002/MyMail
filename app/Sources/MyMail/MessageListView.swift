import SwiftUI

struct MessageListView: View {
    @EnvironmentObject private var store: MailStore
    @Environment(\.openWindow) private var openWindow

    private var folder: Folder { store.selectedFolder }

    private var subtitle: String {
        let count = store.folderCounts[folder]
        let total = count?.total ?? store.messages.count
        let unread = count?.unread ?? 0
        var text = total == 1 ? "1 message" : "\(total) messages"
        if unread > 0 && folder.showsUnreadBadge { text += ", \(unread) unread" }
        return text
    }

    var body: some View {
        List(store.messages, selection: $store.selectedMessageId) { message in
            MessageRow(message: message, showRecipient: folder == .sent || folder == .drafts)
                .tag(message.id)
                .contextMenu { contextMenu(for: message) }
                .swipeActions(edge: .leading) {
                    Button {
                        Task { await store.setRead(message.id, !message.isRead) }
                    } label: {
                        Label(message.isRead ? "Unread" : "Read",
                              systemImage: message.isRead ? "envelope.badge" : "envelope.open")
                    }
                    .tint(.blue)
                }
                .swipeActions(edge: .trailing) {
                    Button(role: .destructive) {
                        Task { await store.delete(message.id) }
                    } label: {
                        Label("Delete", systemImage: "trash")
                    }
                    if canArchive(message) {
                        Button {
                            Task { await store.move(message.id, to: .archive) }
                        } label: {
                            Label("Archive", systemImage: "archivebox")
                        }
                        .tint(.purple)
                    }
                }
        }
        .listStyle(.inset)
        .overlay { emptyState }
        .searchable(text: $store.searchText, placement: .toolbar, prompt: "Search \(folder.title)")
        .navigationTitle(folder.title)
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItemGroup {
                Button {
                    Task { await store.refresh() }
                } label: {
                    Label("Get New Mail", systemImage: "arrow.clockwise")
                }
                .help("Get new mail (⇧⌘N)")

                Button {
                    if let state = store.newCompose() { openWindow(value: state) }
                } label: {
                    Label("New Message", systemImage: "square.and.pencil")
                }
                .help("New message (⌘N)")
            }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if store.messages.isEmpty {
            if store.isLoadingMessages {
                ProgressView()
            } else if !store.searchText.trimmingCharacters(in: .whitespaces).isEmpty {
                ContentUnavailableView.search(text: store.searchText)
            } else {
                ContentUnavailableView(
                    "No Messages",
                    systemImage: folder.symbol,
                    description: Text(emptyDescription)
                )
            }
        }
    }

    private var emptyDescription: String {
        switch folder {
        case .inbox: "You're all caught up."
        case .starred: "Star important messages to find them here."
        case .sent: "Messages you send appear here."
        case .drafts: "Saved drafts appear here."
        case .archive: "Archived messages appear here."
        case .trash: "Trash is empty."
        }
    }

    private func canArchive(_ message: MessageSummary) -> Bool {
        message.folder != Folder.archive.rawValue && message.folder != Folder.drafts.rawValue
    }

    @ViewBuilder
    private func contextMenu(for message: MessageSummary) -> some View {
        Button(message.isRead ? "Mark as Unread" : "Mark as Read") {
            Task { await store.setRead(message.id, !message.isRead) }
        }
        Button(message.isStarred ? "Unstar" : "Star") {
            Task { await store.toggleStar(message.id) }
        }
        Divider()
        if canArchive(message) {
            Button("Archive") { Task { await store.move(message.id, to: .archive) } }
        }
        if message.folder == Folder.archive.rawValue || message.folder == Folder.trash.rawValue {
            Button("Move to Inbox") { Task { await store.move(message.id, to: .inbox) } }
        }
        Button(message.folder == Folder.trash.rawValue ? "Delete Permanently" : "Delete", role: .destructive) {
            Task { await store.delete(message.id) }
        }
    }
}

struct MessageRow: View {
    let message: MessageSummary
    let showRecipient: Bool

    private var title: String {
        if showRecipient {
            let to = message.toAddrs.isEmpty ? "(no recipients)" : message.toAddrs
            return "To: \(to)"
        }
        return message.senderDisplay
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(message.isRead ? Color.clear : Color.accentColor)
                .frame(width: 8, height: 8)
                .padding(.top, 5)
                .accessibilityLabel(message.isRead ? "" : "Unread")

            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(title)
                        .font(.headline.weight(message.isRead ? .medium : .bold))
                        .lineLimit(1)
                    Spacer(minLength: 6)
                    if message.isStarred {
                        Image(systemName: "star.fill")
                            .font(.caption)
                            .foregroundStyle(.yellow)
                            .accessibilityLabel("Starred")
                    }
                    Text(RelativeDate.short(message.createdAt))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                Text(message.subject)
                    .font(.subheadline.weight(message.isRead ? .regular : .semibold))
                    .lineLimit(1)
                if !message.snippet.isEmpty {
                    Text(message.snippet)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .padding(.vertical, 5)
    }
}

enum RelativeDate {
    /// "14:05" today, "Yesterday", weekday within a week, else a short date.
    static func short(_ date: Date, now: Date = .now) -> String {
        let cal = Calendar.current
        if cal.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        if cal.isDateInYesterday(date) {
            return "Yesterday"
        }
        if let days = cal.dateComponents([.day], from: date, to: now).day, days < 7, days >= 0 {
            return date.formatted(.dateTime.weekday(.wide))
        }
        if cal.isDate(date, equalTo: now, toGranularity: .year) {
            return date.formatted(.dateTime.month(.abbreviated).day())
        }
        return date.formatted(date: .numeric, time: .omitted)
    }
}
