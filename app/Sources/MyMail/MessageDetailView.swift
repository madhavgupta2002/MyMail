import SwiftUI

struct MessageDetailView: View {
    @EnvironmentObject private var store: MailStore
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        if let message = store.selectedMessage {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(message.subject)
                        .font(.title2.weight(.semibold))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)

                    header(message)

                    Divider()

                    Text(message.body.isEmpty ? " " : message.body)
                        .font(.body)
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(24)
                .frame(maxWidth: 820, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toolbar { toolbar(message) }
            .navigationTitle("")
        } else {
            ContentUnavailableView(
                "No Message Selected",
                systemImage: "envelope",
                description: Text(store.messages.isEmpty ? "" : "Select a message to read it.")
            )
            .toolbar {
                // Keeps the toolbar layout stable when nothing is selected.
                ToolbarItem { Color.clear.frame(width: 1, height: 1) }
            }
        }
    }

    private func header(_ message: Message) -> some View {
        HStack(alignment: .top, spacing: 12) {
            AvatarView(name: message.fromName, address: message.fromAddr, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(message.senderDisplay)
                        .font(.headline)
                    Text("<\(message.fromAddr)>")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .textSelection(.enabled)
                if !message.toAddrs.isEmpty {
                    addressLine("To", message.toAddrs)
                }
                if !message.ccAddrs.isEmpty {
                    addressLine("Cc", message.ccAddrs)
                }
            }
            Spacer(minLength: 12)
            VStack(alignment: .trailing, spacing: 4) {
                Text(message.createdAt.formatted(date: .abbreviated, time: .shortened))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                if message.isStarred {
                    Image(systemName: "star.fill")
                        .foregroundStyle(.yellow)
                        .accessibilityLabel("Starred")
                }
            }
        }
    }

    private func addressLine(_ label: String, _ value: String) -> some View {
        (Text("\(label): ").foregroundStyle(.secondary) + Text(value))
            .font(.subheadline)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }

    @ToolbarContentBuilder
    private func toolbar(_ message: Message) -> some ToolbarContent {
        let isDraft = message.folder == Folder.drafts.rawValue
        if isDraft {
            ToolbarItem {
                Button {
                    if let s = store.editDraftCompose() { openWindow(value: s) }
                } label: {
                    Label("Edit Draft", systemImage: "square.and.pencil")
                }
                .help("Continue editing this draft")
            }
        } else {
            ToolbarItemGroup {
                Button {
                    if let s = store.replyCompose(all: false) { openWindow(value: s) }
                } label: {
                    Label("Reply", systemImage: "arrowshape.turn.up.left")
                }
                .help("Reply (⌘R)")

                Button {
                    if let s = store.replyCompose(all: true) { openWindow(value: s) }
                } label: {
                    Label("Reply All", systemImage: "arrowshape.turn.up.left.2")
                }
                .help("Reply All (⇧⌘R)")

                Button {
                    if let s = store.forwardCompose() { openWindow(value: s) }
                } label: {
                    Label("Forward", systemImage: "arrowshape.turn.up.right")
                }
                .help("Forward (⇧⌘F)")
            }
        }

        ToolbarItemGroup {
            if !isDraft {
                Button {
                    Task { await store.toggleStar(message.id) }
                } label: {
                    Label(message.isStarred ? "Unstar" : "Star",
                          systemImage: message.isStarred ? "star.fill" : "star")
                }
                .help(message.isStarred ? "Unstar (⇧⌘L)" : "Star (⇧⌘L)")

                Button {
                    Task { await store.setRead(message.id, false) }
                } label: {
                    Label("Mark as Unread", systemImage: "envelope.badge")
                }
                .help("Mark as Unread (⇧⌘U)")

                if message.folder == Folder.archive.rawValue || message.folder == Folder.trash.rawValue {
                    Button {
                        Task { await store.move(message.id, to: .inbox) }
                    } label: {
                        Label("Move to Inbox", systemImage: "tray.and.arrow.down")
                    }
                    .help("Move to Inbox")
                } else {
                    Button {
                        Task { await store.move(message.id, to: .archive) }
                    } label: {
                        Label("Archive", systemImage: "archivebox")
                    }
                    .help("Archive (⌃⌘A)")
                }
            }

            Button {
                Task { await store.delete(message.id) }
            } label: {
                Label("Delete", systemImage: "trash")
            }
            .help(message.folder == Folder.trash.rawValue || isDraft ? "Delete Permanently (⌘⌫)" : "Delete (⌘⌫)")
        }
    }
}
