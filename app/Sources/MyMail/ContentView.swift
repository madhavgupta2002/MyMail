import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var store: MailStore

    var body: some View {
        Group {
            switch store.connection {
            case .connecting:
                ProgressView("Connecting to server…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .failed(let reason):
                ContentUnavailableView {
                    Label("Can't Connect to Mail Server", systemImage: "bolt.horizontal.circle")
                } description: {
                    Text(reason)
                        .textSelection(.enabled)
                } actions: {
                    Button("Try Again") { Task { await store.start() } }
                        .keyboardShortcut(.defaultAction)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .ready:
                NavigationSplitView {
                    SidebarView()
                        .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 300)
                } content: {
                    MessageListView()
                        .navigationSplitViewColumnWidth(min: 280, ideal: 360, max: 520)
                } detail: {
                    MessageDetailView()
                }
            }
        }
        .overlay(alignment: .bottom) {
            if let banner = store.banner {
                BannerView(banner: banner) { store.banner = nil }
                    .padding(.bottom, 16)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.snappy, value: store.banner)
        .task { await store.start() }
    }
}

struct BannerView: View {
    let banner: Banner
    let onDismiss: () -> Void

    private var symbol: String {
        switch banner.kind {
        case .info: "checkmark.circle.fill"
        case .warning: "exclamationmark.triangle.fill"
        case .error: "xmark.octagon.fill"
        }
    }

    private var tint: Color {
        switch banner.kind {
        case .info: .green
        case .warning: .orange
        case .error: .red
        }
    }

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
            Text(banner.text)
                .lineLimit(3)
                .textSelection(.enabled)
            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Dismiss")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(.separator))
        .shadow(color: .black.opacity(0.12), radius: 8, y: 3)
        .frame(maxWidth: 560)
    }
}

/// Circular initials avatar with a stable per-address color.
struct AvatarView: View {
    let name: String
    let address: String
    var size: CGFloat = 36

    private var color: Color {
        let hash = address.lowercased().unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0xFFFF }
        return Color(hue: Double(hash % 360) / 360, saturation: 0.5, brightness: 0.8)
    }

    var body: some View {
        Circle()
            .fill(color.gradient)
            .frame(width: size, height: size)
            .overlay(
                Text(Initials.of(name.isEmpty ? address : name))
                    .font(.system(size: size * 0.4, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
            )
            .accessibilityHidden(true)
    }
}
