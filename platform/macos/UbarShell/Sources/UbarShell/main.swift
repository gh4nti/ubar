import AppKit
import SwiftUI
import UniformTypeIdentifiers

extension Notification.Name {
    static let openBrowserManager = Notification.Name("openBrowserManager")
}

@main
struct UbarApp: App {
    @State private var privateMode = CommandLine.arguments.contains("--incognito")

    var body: some Scene {
        WindowGroup(privateMode ? "uBar Private" : "uBar") {
            BrowserWindow(privateMode: privateMode)
                .frame(minWidth: 900, minHeight: 620)
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) {}
            CommandMenu("Browser") {
                Button("History") { openManager("history") }
                    .keyboardShortcut("y", modifiers: .command)
                Button("Settings") { openManager("settings") }
                    .keyboardShortcut(",", modifiers: .command)
                Button("Bookmarks") { openManager("bookmarks") }
                    .keyboardShortcut("o", modifiers: [.command, .shift])
                Button("Extensions") { openManager("extensions") }
                    .keyboardShortcut("x", modifiers: [.command, .shift])
            }
        }
    }

    private func openManager(_ manager: String) {
        NotificationCenter.default.post(name: .openBrowserManager, object: manager)
    }
}

struct BrowserTab: Identifiable {
    let id = UUID()
    var title = "New Tab"
    var zoom = 1.0
}

struct BrowserWindow: View {
    let privateMode: Bool
    @State private var address = ""
    @State private var tabs = [BrowserTab()]
    @State private var selection: UUID?
    @State private var extensionMessage: String?
    @State private var extensionActions: [ShellExtensionAction] = []
    @State private var choosingExtension = false
    @State private var pendingExtension: URL?
    @State private var engine = EngineBridge.load()

    var body: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                Button(action: { if let selection { engine.goBack(tab: selection) } }) {
                    Image(systemName: "chevron.left")
                }
                Button(action: { if let selection { engine.goForward(tab: selection) } }) {
                    Image(systemName: "chevron.right")
                }
                Button(action: { if let selection { engine.reload(tab: selection) } }) {
                    Image(systemName: "arrow.clockwise")
                }
                TextField("Search or enter address", text: $address)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { if let selection { engine.navigate(address, tab: selection) } }
                ForEach(extensionActions) { action in
                    Button(action.title) {
                        if let popup = engine.extensionPopup(id: action.id), let selection {
                            address = popup
                            engine.navigate(popup, tab: selection)
                        } else {
                            extensionMessage = "Extension action has no popup."
                        }
                    }
                    .help(action.title)
                }
                Button(action: bookmarkCurrent) { Image(systemName: "star") }
                Button("Library") { extensionMessage = engine.librarySummary() }
                Button("Extensions +") { choosingExtension = true }
                Button(action: { changeZoom(-0.1) }) { Image(systemName: "minus.magnifyingglass") }
                Button(action: { changeZoom(0.1) }) { Image(systemName: "plus.magnifyingglass") }
                if privateMode { Label("Private", systemImage: "hand.raised.fill") }
                else { Button("Private", action: openPrivateWindow) }
                Button(action: addTab) { Image(systemName: "plus") }
                Button(action: closeSelectedTab) { Image(systemName: "xmark") }
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)

            TabView(selection: $selection) {
                ForEach(tabs) { tab in
                    EngineCanvas(tab: tab.id, engine: engine, visible: selection == tab.id)
                        .tabItem { Text(tab.title) }
                        .tag(Optional(tab.id))
                }
            }
        }
        .background(.ultraThinMaterial)
        .onAppear {
            selection = selection ?? tabs.first?.id
            extensionActions = engine.extensionActions
            engine.onEvent = handleEngineEvent
        }
        .onReceive(NotificationCenter.default.publisher(for: .openBrowserManager)) { notification in
            guard let manager = notification.object as? String else { return }
            extensionMessage = engine.managerSummary(manager)
        }
        .fileImporter(
            isPresented: $choosingExtension,
            allowedContentTypes: [
                UTType(filenameExtension: "xpi") ?? .data,
                UTType(filenameExtension: "crx") ?? .data,
            ]
        ) { result in
            guard case .success(let url) = result else { return }
            let access = url.startAccessingSecurityScopedResource()
            defer { if access { url.stopAccessingSecurityScopedResource() } }
            if engine.installExtension(at: url) {
                extensionActions = engine.extensionActions
            } else {
                extensionMessage = "Package signature, integrity, or compatibility validation failed."
            }
        }
        .alert("uBar", isPresented: Binding(
            get: { extensionMessage != nil },
            set: { if !$0 { extensionMessage = nil } }
        )) { Button("Close", role: .cancel) { extensionMessage = nil } }
        message: { Text(extensionMessage ?? "") }
        .alert("Install extension?", isPresented: Binding(
            get: { pendingExtension != nil },
            set: { if !$0 { discardPendingExtension() } }
        )) {
            Button("Cancel", role: .cancel) { discardPendingExtension() }
            Button("Install") {
                guard let url = pendingExtension else { return }
                if engine.installExtension(at: url) { extensionActions = engine.extensionActions }
                else { extensionMessage = "Package signature, integrity, or compatibility validation failed." }
                discardPendingExtension()
            }
        } message: { Text("Only store-signed Firefox or Chrome packages pass verification.") }
    }

    private func openPrivateWindow() {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.arguments = ["--incognito"]
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration)
    }

    private func addTab() {
        let tab = BrowserTab()
        tabs.append(tab)
        selection = tab.id
        address = ""
    }

    private func bookmarkCurrent() {
        guard !privateMode, let selection,
              let tab = tabs.first(where: { $0.id == selection }),
              URL(string: address)?.scheme != nil else { return }
        if engine.addBookmark(title: tab.title, url: address) { extensionMessage = "Bookmarked \(tab.title)" }
    }

    private func closeSelectedTab() {
        guard let selection, let index = tabs.firstIndex(where: { $0.id == selection }) else { return }
        tabs.remove(at: index)
        if tabs.isEmpty { addTab() }
        else { self.selection = tabs[min(index, tabs.count - 1)].id }
    }

    private func changeZoom(_ delta: Double) {
        guard let selection, let index = tabs.firstIndex(where: { $0.id == selection }) else { return }
        tabs[index].zoom = min(5, max(0.5, ((tabs[index].zoom + delta) * 10).rounded() / 10))
        engine.setZoom(tab: selection, zoom: tabs[index].zoom)
    }

    private func handleEngineEvent(_ event: ShellEngineEvent) {
        if event.kind == 7 && !event.text.isEmpty {
            let url = URL(fileURLWithPath: event.text)
            if privateMode { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            else { pendingExtension = url }
            return
        }
        guard let tab = tabs.firstIndex(where: { engine.view(for: $0.id) == event.view }) else { return }
        if event.kind == 3 && !event.text.isEmpty { tabs[tab].title = event.text }
        else if event.kind == 4 && tabs[tab].id == selection { address = event.text }
        else if event.kind == 5 { tabs[tab].title = "Crashed" }
    }

    private func discardPendingExtension() {
        guard let url = pendingExtension else { return }
        try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
        pendingExtension = nil
    }
}

struct EngineCanvas: NSViewRepresentable {
    let tab: UUID
    let engine: EngineBridge
    let visible: Bool

    final class Coordinator {
        var tab: UUID?
        weak var engine: EngineBridge?
    }
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        view.wantsLayer = true
        context.coordinator.tab = tab
        context.coordinator.engine = engine
        engine.attach(tab: tab, to: view)
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        engine.setVisible(tab: tab, visible)
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        if let tab = coordinator.tab { coordinator.engine?.detach(tab: tab) }
    }
}
