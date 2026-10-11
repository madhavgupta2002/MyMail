import SwiftUI

struct ComposeView: View {
    @EnvironmentObject private var store: MailStore
    @Environment(\.dismiss) private var dismiss

    @State private var compose: Compose
    @State private var draftId: Int64?
    @State private var sending = false
    @State private var savingDraft = false
    @State private var error: String?
    @State private var savedNotice: String?
    @State private var showCc: Bool
    @FocusState private var focus: Field?

    private enum Field { case to, cc, subject, body }

    init(initial: ComposeState) {
        _compose = State(initialValue: initial.compose)
        _draftId = State(initialValue: initial.draftId)
        _showCc = State(initialValue: !initial.compose.cc.isEmpty)
    }

    private var hasRecipients: Bool {
        !splitAddresses(compose.to).isEmpty || !splitAddresses(compose.cc).isEmpty
    }

    private var windowTitle: String {
        let subject = compose.subject.trimmingCharacters(in: .whitespaces)
        return subject.isEmpty ? "New Message" : subject
    }

    var body: some View {
        VStack(spacing: 0) {
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 8, verticalSpacing: 0) {
                if store.accounts.count > 1 {
                    fieldRow("From:") {
                        Picker("From", selection: $compose.accountId) {
                            ForEach(store.accounts) { account in
                                Text("\(account.name) <\(account.email)>").tag(account.id)
                            }
                        }
                        .labelsHidden()
                        .fixedSize()
                        .disabled(draftId != nil)
                        Spacer()
                    }
                }
                fieldRow("To:") {
                    TextField("", text: $compose.to, prompt: Text("name@mymail.local"))
                        .focused($focus, equals: .to)
                    if !showCc {
                        Button("Cc") {
                            showCc = true
                            focus = .cc
                        }
                        .buttonStyle(.link)
                        .font(.callout)
                    }
                }
                if showCc {
                    fieldRow("Cc:") {
                        TextField("", text: $compose.cc)
                            .focused($focus, equals: .cc)
                    }
                }
                fieldRow("Subject:") {
                    TextField("", text: $compose.subject)
                        .focused($focus, equals: .subject)
                }
            }
            .textFieldStyle(.plain)
            .padding(.horizontal, 16)

            TextEditor(text: $compose.body)
                .font(.body)
                .scrollContentBackground(.hidden)
                .focused($focus, equals: .body)
                .padding(.horizontal, 11)
                .padding(.vertical, 10)

            if let error {
                statusBar(error, symbol: "exclamationmark.triangle.fill", tint: .red)
            } else if let savedNotice {
                statusBar(savedNotice, symbol: "checkmark.circle.fill", tint: .green)
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
        .navigationTitle(windowTitle)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button(role: .destructive) {
                    Task { await discard() }
                } label: {
                    Label("Discard", systemImage: "trash")
                }
                .help(draftId == nil ? "Discard message" : "Delete draft")

                Button {
                    Task { await saveDraft() }
                } label: {
                    Label("Save Draft", systemImage: "square.and.arrow.down")
                }
                .keyboardShortcut("s")
                .help("Save Draft (⌘S)")
                .disabled(savingDraft || sending)

                Button {
                    Task { await send() }
                } label: {
                    if sending {
                        ProgressView().controlSize(.small)
                    } else {
                        Label("Send", systemImage: "paperplane.fill")
                    }
                }
                .keyboardShortcut(.return, modifiers: .command)
                .help("Send (⌘↩)")
                .disabled(!hasRecipients || sending)
            }
        }
        .onAppear {
            focus = compose.to.isEmpty ? .to : .body
        }
        .onChange(of: compose) {
            error = nil
            savedNotice = nil
        }
    }

    @ViewBuilder
    private func fieldRow<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
            HStack { content() }
        }
        .padding(.vertical, 8)
        Divider()
            .gridCellUnsizedAxes(.horizontal)
    }

    private func statusBar(_ text: String, symbol: String, tint: Color) -> some View {
        HStack(spacing: 6) {
            Image(systemName: symbol).foregroundStyle(tint)
            Text(text)
                .font(.callout)
                .textSelection(.enabled)
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private func send() async {
        guard !sending else { return }
        sending = true
        defer { sending = false }
        var message = compose
        message.draftId = draftId
        do {
            try await store.send(message)
            dismiss()
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func saveDraft() async {
        savingDraft = true
        defer { savingDraft = false }
        do {
            draftId = try await store.saveDraft(compose, id: draftId)
            savedNotice = "Draft saved"
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func discard() async {
        if let draftId {
            await store.delete(draftId)
        }
        dismiss()
    }
}
