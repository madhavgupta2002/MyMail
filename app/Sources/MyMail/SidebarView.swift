import SwiftUI

struct SidebarView: View {
    @EnvironmentObject private var store: MailStore
    @State private var showingNewAccount = false
    @State private var confirmingEmptyTrash = false

    private var folderSelection: Binding<Folder?> {
        Binding(
            get: { store.selectedFolder },
            set: { if let folder = $0 { store.selectedFolder = folder } }
        )
    }

    var body: some View {
        List(selection: folderSelection) {
            Section("Mailboxes") {
                ForEach(Folder.allCases) { folder in
                    folderRow(folder)
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .top, spacing: 0) {
            accountSwitcher
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
        }
        .sheet(isPresented: $showingNewAccount) {
            NewAccountView()
        }
        .confirmationDialog(
            "Permanently delete all messages in Trash?",
            isPresented: $confirmingEmptyTrash
        ) {
            Button("Empty Trash", role: .destructive) { Task { await store.emptyTrash() } }
        } message: {
            Text("This can't be undone.")
        }
    }

    @ViewBuilder
    private func folderRow(_ folder: Folder) -> some View {
        let count = store.folderCounts[folder]
        let badge: Int = {
            guard let count else { return 0 }
            if folder == .drafts { return count.total }
            return folder.showsUnreadBadge ? count.unread : 0
        }()
        Label(folder.title, systemImage: folder.symbol)
            .badge(badge)
            .tag(folder)
            .contextMenu {
                if folder != .drafts && folder != .sent {
                    Button("Mark All as Read") { Task { await store.markAllRead(in: folder) } }
                        .disabled((count?.unread ?? 0) == 0)
                }
                if folder == .trash {
                    Button("Empty Trash…") { confirmingEmptyTrash = true }
                        .disabled((count?.total ?? 0) == 0)
                }
            }
    }

    private var accountSwitcher: some View {
        Menu {
            Section("Accounts") {
                ForEach(store.accounts) { account in
                    Button {
                        store.selectedAccountId = account.id
                    } label: {
                        if account.id == store.selectedAccountId {
                            Label("\(account.name) — \(account.email)", systemImage: "checkmark")
                        } else {
                            Text("\(account.name) — \(account.email)")
                        }
                    }
                }
            }
            Divider()
            Button("New Account…") { showingNewAccount = true }
        } label: {
            HStack(spacing: 8) {
                if let account = store.selectedAccount {
                    AvatarView(name: account.name, address: account.email, size: 28)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(account.name)
                            .font(.headline)
                            .lineLimit(1)
                        Text(account.email)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                } else {
                    Text("No Account")
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .contentShape(Rectangle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .padding(8)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        .help("Switch account")
    }
}

struct NewAccountView: View {
    @EnvironmentObject private var store: MailStore
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var email = ""
    @State private var error: String?
    @State private var saving = false

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty && email.contains("@") && !saving
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("New Account")
                .font(.title2.bold())
            Text("Accounts are local mailboxes. Mail sent between local accounts is delivered instantly.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Form {
                TextField("Name", text: $name, prompt: Text("Jane Appleseed"))
                TextField("Email", text: $email, prompt: Text("jane@mymail.local"))
                    .textContentType(.emailAddress)
            }
            .formStyle(.columns)
            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .font(.callout)
            }
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create") { Task { await save() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canSave)
            }
        }
        .padding(20)
        .frame(width: 400)
    }

    private func save() async {
        saving = true
        defer { saving = false }
        do {
            try await store.createAccount(name: name, email: email)
            dismiss()
        } catch {
            self.error = error.localizedDescription
        }
    }
}
