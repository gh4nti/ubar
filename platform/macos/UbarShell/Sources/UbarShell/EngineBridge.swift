import CEngine
import Foundation
import AppKit

final class EngineBridge {
    private var handle: OpaquePointer?
    private var views: [UUID: UInt64] = [:]
    private(set) var status: String
    private(set) var extensionActions: [ShellExtensionAction] = []
    private var memoryTimer: Timer?
    var onEvent: ((ShellEngineEvent) -> Void)?

    static func load() -> EngineBridge {
        let privateMode = CommandLine.arguments.contains("--incognito")
        return EngineBridge(privateMode: privateMode)
    }

    private init(privateMode: Bool) {
        handle = ubar_shell_engine_open(privateMode, ProcessInfo.processInfo.physicalMemory)
        status = handle.map { String(cString: ubar_shell_engine_status($0)) }
            ?? "engine ABI unavailable"
        extensionActions = extensionControl(["operation": "list"], as: [ShellExtensionAction].self) ?? []
        ubar_shell_engine_set_event_callback(handle, { context, kind, view, text, value in
            guard let context else { return }
            let bridge = Unmanaged<EngineBridge>.fromOpaque(context).takeUnretainedValue()
            let event = ShellEngineEvent(
                kind: kind, view: view, text: text.map(String.init(cString:)) ?? "", value: value)
            DispatchQueue.main.async { bridge.onEvent?(event) }
        }, Unmanaged.passUnretained(self).toOpaque())
        memoryTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            self?.reportMemory()
        }
        memoryTimer?.tolerance = 1
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

    func view(for tab: UUID) -> UInt64? { views[tab] }

    func navigate(_ input: String, tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        normalized(input).withCString { ubar_shell_engine_navigate(handle, view, $0) }
    }

    func goBack(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_go_back(handle, view)
    }

    func goForward(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_go_forward(handle, view)
    }

    func reload(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_reload(handle, view)
    }

    func stop(tab: UUID) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_stop(handle, view)
    }

    func setZoom(tab: UUID, zoom: Double) {
        guard let handle, let view = views[tab] else { return }
        ubar_shell_engine_set_zoom(handle, view, min(5, max(0.5, zoom)))
    }

    private func normalized(_ raw: String) -> String {
        let input = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if URL(string: input)?.scheme != nil { return input }
        if input.contains(where: { $0.isWhitespace }) || !input.contains(".") {
            var components = URLComponents(string: "https://www.google.com/search")!
            components.queryItems = [URLQueryItem(name: "q", value: input)]
            return components.url!.absoluteString
        }
        return "https://\(input)"
    }

    func extensionPopup(id: String) -> String? {
        let action: ShellExtensionActionResult? = extensionControl(
            ["operation": "getAction", "id": id], as: ShellExtensionActionResult.self)
        return action?.popup
    }

    func installExtension(at url: URL) -> Bool {
        let result: ShellExtensionAction? = extensionControl(
            ["operation": "installPackage", "path": url.path], as: ShellExtensionAction.self)
        if result != nil {
            extensionActions = extensionControl(["operation": "list"], as: [ShellExtensionAction].self) ?? []
        }
        return result != nil
    }

    func addBookmark(title: String, url: String) -> Bool {
        let result: ShellBrowserAck? = browserControl([
            "method": "addBookmark", "title": title, "url": url, "folder": "",
            "createdAtMs": Int64(Date().timeIntervalSince1970 * 1000),
        ], as: ShellBrowserAck.self)
        return result != nil
    }

    private func reportMemory() {
        let _: ShellMemoryReport? = browserControl([
            "method": "reportMemory", "residentBytes": 0,
        ], as: ShellMemoryReport.self)
    }

    func librarySummary() -> String {
        let bookmarks: [ShellBookmark] = browserControl(["method": "listBookmarks"], as: [ShellBookmark].self) ?? []
        let history: [ShellHistory] = browserControl(
            ["method": "queryHistory", "limit": 20], as: [ShellHistory].self) ?? []
        let bookmarkLines = bookmarks.prefix(20).map { "★ \($0.title) — \($0.url)" }.joined(separator: "\n")
        let historyLines = history.prefix(20).map { "• \($0.title) — \($0.url)" }.joined(separator: "\n")
        return "Bookmarks\n\(bookmarkLines.isEmpty ? "None" : bookmarkLines)\n\nRecent history\n\(historyLines.isEmpty ? "None" : historyLines)"
    }

    func managerSummary(_ manager: String) -> String {
        switch manager {
        case "history":
            let items: [ShellHistory] = browserControl(
                ["method": "queryHistory", "limit": 100], as: [ShellHistory].self) ?? []
            let lines = items.map { "• \($0.title) — \($0.url)" }.joined(separator: "\n")
            return "History\n\(lines.isEmpty ? "No history yet." : lines)"
        case "bookmarks":
            let items: [ShellBookmark] = browserControl(
                ["method": "listBookmarks"], as: [ShellBookmark].self) ?? []
            let lines = items.map { "★ \($0.title) — \($0.url)" }.joined(separator: "\n")
            return "Bookmarks\n\(lines.isEmpty ? "No bookmarks yet." : lines)"
        case "extensions":
            let lines = extensionActions.map { "• \($0.title)" }.joined(separator: "\n")
            return "Extensions\n\(lines.isEmpty ? "No extensions installed." : lines)"
        case "settings":
            let settings: [String: JSONValue] = browserControl(
                ["method": "getSettings"], as: [String: JSONValue].self) ?? [:]
            let lines = settings.sorted { $0.key < $1.key }
                .map { "\($0.key): \($0.value.description)" }.joined(separator: "\n")
            return "Settings\n\(lines.isEmpty ? "Using defaults." : lines)"
        default:
            return "Unknown browser manager."
        }
    }

    private func browserControl<T: Decodable>(_ request: [String: Any], as type: T.Type) -> T? {
        guard let handle,
              let data = try? JSONSerialization.data(withJSONObject: request),
              let text = String(data: data, encoding: .utf8) else { return nil }
        return text.withCString { request in
            guard let result = ubar_shell_engine_browser_control(handle, request) else { return nil }
            defer { ubar_shell_engine_free_string(result) }
            return String(cString: result).data(using: .utf8).flatMap { try? JSONDecoder().decode(type, from: $0) }
        }
    }

    private func extensionControl<T: Decodable>(_ request: [String: String], as type: T.Type) -> T? {
        guard let handle,
              let data = try? JSONSerialization.data(withJSONObject: request),
              let text = String(data: data, encoding: .utf8) else { return nil }
        return text.withCString { request in
            guard let result = ubar_shell_engine_extension_control(handle, request) else { return nil }
            defer { ubar_shell_engine_free_string(result) }
            return String(cString: result).data(using: .utf8).flatMap { try? JSONDecoder().decode(type, from: $0) }
        }
    }

    deinit {
        memoryTimer?.invalidate()
        if let handle {
            for view in views.values { ubar_shell_engine_destroy_view(handle, view) }
            ubar_shell_engine_close(handle)
        }
    }
}

struct ShellExtensionAction: Decodable, Identifiable {
    struct Action: Decodable { let default_title: String? }
    let id: String
    let name: String
    let action: Action?
    var title: String { action?.default_title ?? name }
}

struct ShellEngineEvent { let kind: UInt32; let view: UInt64; let text: String; let value: UInt64 }

private struct ShellExtensionActionResult: Decodable { let popup: String? }
private struct ShellBrowserAck: Decodable { let id: String }
private struct ShellBookmark: Decodable { let title: String; let url: String }
private struct ShellHistory: Decodable { let title: String; let url: String }
private struct ShellMemoryReport: Decodable { let residentBytes: UInt64; let hibernated: Int }

private enum JSONValue: Decodable, CustomStringConvertible {
    case string(String), number(Double), boolean(Bool), object, array, null

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let item = try? value.decode(String.self) { self = .string(item) }
        else if let item = try? value.decode(Bool.self) { self = .boolean(item) }
        else if let item = try? value.decode(Double.self) { self = .number(item) }
        else if (try? value.decode([String: JSONValue].self)) != nil { self = .object }
        else { self = .array }
    }

    var description: String {
        switch self {
        case .string(let value): value
        case .number(let value): String(value)
        case .boolean(let value): String(value)
        case .object: "{...}"
        case .array: "[...]"
        case .null: "null"
        }
    }
}
