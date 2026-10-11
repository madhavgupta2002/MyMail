import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var signalSources: [DispatchSourceSignal] = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Needed when launched as a bare SwiftPM executable (no .app bundle).
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()

        // Route SIGTERM/SIGINT through a normal quit so the spawned server is stopped.
        for sig in [SIGTERM, SIGINT] {
            signal(sig, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }
            source.resume()
            signalSources.append(source)
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        ServerManager.shared.stop()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

@main
struct MyMailApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var store = MailStore()

    var body: some Scene {
        Window("MyMail", id: "main") {
            ContentView()
                .environmentObject(store)
                .focusedSceneObject(store)
                .frame(minWidth: 900, minHeight: 540)
        }
        .defaultSize(width: 1180, height: 740)
        .commands { MailCommands(store: store) }

        WindowGroup("New Message", id: "compose", for: ComposeState.self) { $state in
            if let state {
                ComposeView(initial: state)
                    .environmentObject(store)
                    .frame(minWidth: 520, minHeight: 420)
            }
        }
        .defaultSize(width: 640, height: 560)
        .commandsRemoved()
    }
}

struct MailCommands: Commands {
    @ObservedObject var store: MailStore
    /// Only set while the main window is key, so message shortcuts
    /// (e.g. ⌘⌫) don't fire while typing in a compose window.
    @FocusedObject private var focused: MailStore?
    @Environment(\.openWindow) private var openWindow

    private var message: Message? { focused?.selectedMessage }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Message") {
                if let state = store.newCompose() { openWindow(value: state) }
            }
            .keyboardShortcut("n")
            .disabled(store.selectedAccountId == nil)
        }

        CommandMenu("Mailbox") {
            Button("Get New Mail") {
                Task { await store.refresh() }
            }
            .keyboardShortcut("n", modifiers: [.command, .shift])

            Divider()

            ForEach(Array(Folder.allCases.enumerated()), id: \.element) { index, folder in
                Button(folder.title) { store.selectedFolder = folder }
                    .keyboardShortcut(KeyEquivalent(Character(String(index + 1))), modifiers: .command)
            }
        }

        CommandMenu("Message") {
            Button("Reply") {
                if let s = focused?.replyCompose(all: false) { openWindow(value: s) }
            }
            .keyboardShortcut("r")
            .disabled(!canRespond)

            Button("Reply All") {
                if let s = focused?.replyCompose(all: true) { openWindow(value: s) }
            }
            .keyboardShortcut("r", modifiers: [.command, .shift])
            .disabled(!canRespond)

            Button("Forward") {
                if let s = focused?.forwardCompose() { openWindow(value: s) }
            }
            .keyboardShortcut("f", modifiers: [.command, .shift])
            .disabled(!canRespond)

            Divider()

            Button(message?.isStarred == true ? "Unstar" : "Star") {
                if let id = message?.id { Task { await focused?.toggleStar(id) } }
            }
            .keyboardShortcut("l", modifiers: [.command, .shift])
            .disabled(message == nil)

            Button("Mark as Unread") {
                if let id = message?.id { Task { await focused?.setRead(id, false) } }
            }
            .keyboardShortcut("u", modifiers: [.command, .shift])
            .disabled(message == nil)

            Button("Archive") {
                if let id = message?.id { Task { await focused?.move(id, to: .archive) } }
            }
            .keyboardShortcut("a", modifiers: [.command, .control])
            .disabled(message == nil || message?.folder == Folder.archive.rawValue || message?.folder == Folder.drafts.rawValue)

            Button("Delete") {
                if let id = message?.id { Task { await focused?.delete(id) } }
            }
            .keyboardShortcut(.delete, modifiers: .command)
            .disabled(message == nil)
        }
    }

    private var canRespond: Bool {
        guard let message else { return false }
        return message.folder != Folder.drafts.rawValue
    }
}
