import AppKit
import SwiftUI

@main
struct UbarApp: App {
    @State private var privateMode = CommandLine.arguments.contains("--incognito")

    var body: some Scene {
        WindowGroup(privateMode ? "uBar Private" : "uBar") {
            BrowserWindow(privateMode: privateMode)
                .frame(minWidth: 900, minHeight: 620)
        }
        .windowStyle(.hiddenTitleBar)
        .commands { CommandGroup(replacing: .newItem) {} }
    }
}

struct BrowserTab: Identifiable {
    let id = UUID()
    var title = "New Tab"
}

struct BrowserWindow: View {
    let privateMode: Bool
    @State private var address = ""
    @State private var tabs = [BrowserTab()]
    @State private var selection: UUID?
    private let engine = EngineBridge.load()

    var body: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                Button(action: { if let selection { engine.goBack(tab: selection) } }) {
                    Image(systemName: "chevron.left")
                }
                Button(action: { if let selection { engine.goForward(tab: selection) } }) {
                    Image(systemName: "chevron.right")
                }
                TextField("Search or enter address", text: $address)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { if let selection { engine.navigate(address, tab: selection) } }
                if privateMode { Label("Private", systemImage: "hand.raised.fill") }
                else { Button("Private", action: openPrivateWindow) }
                Button(action: { tabs.append(BrowserTab()) }) { Image(systemName: "plus") }
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
        .onAppear { selection = selection ?? tabs.first?.id }
    }

    private func openPrivateWindow() {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.arguments = ["--incognito"]
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration)
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
