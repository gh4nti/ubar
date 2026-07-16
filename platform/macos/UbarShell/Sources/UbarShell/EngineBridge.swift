import CEngine
import Foundation
import AppKit

final class EngineBridge {
    private var handle: OpaquePointer?
    private var views: [UUID: UInt64] = [:]
    private(set) var status: String

    static func load() -> EngineBridge {
        let privateMode = CommandLine.arguments.contains("--incognito")
        return EngineBridge(privateMode: privateMode)
    }

    private init(privateMode: Bool) {
        handle = ubar_shell_engine_open(privateMode, ProcessInfo.processInfo.physicalMemory)
        status = handle.map { String(cString: ubar_shell_engine_status($0)) }
            ?? "engine ABI unavailable"
    }

    func attach(tab: UUID, to parent: NSView) {
        guard let handle else { return }
        let view = ubar_shell_engine_create_view(handle, Unmanaged.passUnretained(parent).toOpaque())
        if view != 0 { views[tab] = view }
    }

    func detach(tab: UUID) {
        guard let handle, let view = views.removeValue(forKey: tab) else { return }
        ubar_shell_engine_destroy_view(handle, view)
    }

    func setVisible(tab: UUID, _ visible: Bool) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_set_visible(handle, view, visible)
    }

    func navigate(_ input: String, tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        input.withCString { ubar_shell_engine_navigate(handle, view, $0) }
    }

    func goBack(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_go_back(handle, view)
    }

    func goForward(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_go_forward(handle, view)
    }

    deinit {
        if let handle {
            for view in views.values { ubar_shell_engine_destroy_view(handle, view) }
            ubar_shell_engine_close(handle)
        }
    }
}
