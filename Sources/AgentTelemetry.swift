import Foundation
import CoreFoundation

struct TelemetryRecord: Codable, Equatable {
    var id: String
    var title: String
    var value: String
    var detailJSON: String
}

struct TelemetryUsage: Codable, Equatable {
    var input: Int64?
    var output: Int64?
    var reasoning: Int64?
    var cacheRead: Int64?
    var cacheWrite: Int64?
    var total: Int64?
}

struct TelemetryMessage: Codable, Equatable {
    var id: String
    var type: String
    var text: String
    var timestamp: Date?
    var rawJSON: String
}

struct TelemetryToolEvent: Codable, Equatable {
    var id: String
    var name: String
    var status: String
    var inputJSON: String
    var output: String?
    var error: String?
    var startedAt: Date?
    var completedAt: Date?
    var duration: TimeInterval?
    var rawJSON: String
}

struct TelemetrySubagent: Codable, Equatable {
    var id: String
    var title: String
    var sessionID: String?
    var agent: String?
    var status: String?
    var prompt: String?
    var rawJSON: String
}

struct TelemetrySnapshot: Codable, Equatable {
    var sessionID: String
    var title: String
    var project: String
    var collectedAt: Date
    var source = "OpenCode session export"
    var model: String?
    var provider: String?
    var agent: String?
    var parentSessionID: String?
    var taskPrompt: String?
    var usage: TelemetryUsage?
    var recordedCost: Double?
    var costCurrency: String?
    var messages: [TelemetryMessage] = []
    var toolEvents: [TelemetryToolEvent] = []
    var subagents: [TelemetrySubagent] = []
    var context: [TelemetryRecord] = []
    var runtime: [TelemetryRecord] = []
    var warnings: [String] = []
    var error: String?
    var rawInfoJSON = "{}"
}

// Value-only, read-only collection. The caller owns scheduling and stale-project
// rejection. No model, pairing, configuration mutation, or UI work occurs here.
enum AgentTelemetry {
    static func redactedJSON(_ value: Any) -> String { pretty(value) }
    static func redactedText(_ value: String) -> String { clean(value) }

    enum ParseError: LocalizedError {
        case tooLarge, invalidJSON, unsupportedFormat
        var errorDescription: String? {
            switch self {
            case .tooLarge: return "The recorded export exceeds the 32 MiB telemetry limit."
            case .invalidJSON: return "OpenCode returned an incomplete or invalid JSON export."
            case .unsupportedFormat: return "OpenCode returned an unsupported session export format."
            }
        }
    }

    static func collect(project: String, logFile: String) -> TelemetrySnapshot {
        guard !project.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return unavailable(project: project, sessionID: "", error: "Select a project to read recorded telemetry.")
        }
        if let current = try? SessionsPaneData.readCurrentRun(from: URL(fileURLWithPath: logFile)),
           !current.sessionID.isEmpty {
            return collect(project: project, sessionID: current.sessionID)
        }
        let result = run(executable, ["session", "list", "--format", "json", "--max-count", "1"], cwd: project, timeout: 15)
        guard result.code == 0 else {
            return unavailable(project: project, sessionID: "", error: commandFailure(result.code, operation: "list sessions"))
        }
        guard let rows = try? SessionsPaneData.parseSessionsJSON(result.output) else {
            return unavailable(project: project, sessionID: "", error: "OpenCode returned invalid session-list data.")
        }
        guard let row = rows.first else {
            var snapshot = unavailable(project: project, sessionID: "", error: nil)
            snapshot.title = "No recorded session"
            snapshot.warnings = ["No OpenCode session is recorded for this project."]
            return snapshot
        }
        var snapshot = collect(project: project, sessionID: row.id)
        snapshot.source = "OpenCode export · latest recorded session"
        snapshot.warnings.insert("The worker has not recorded a current session ID. Showing the most recent recorded session for this project.", at: 0)
        return snapshot
    }

    static func collect(project: String, sessionID: String) -> TelemetrySnapshot {
        guard !project.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return unavailable(project: project, sessionID: sessionID, error: "Select a project to read recorded telemetry.")
        }
        guard SessionsPaneData.isSessionID(sessionID) else {
            return unavailable(project: project, sessionID: sessionID, error: "A valid recorded session ID is required.")
        }
        let result = run(executable, ["session", "export", sessionID], cwd: project, timeout: 15)
        guard result.code == 0 else {
            return unavailable(project: project, sessionID: sessionID, error: commandFailure(result.code, operation: "export the recorded session"))
        }
        do {
            var snapshot = try parseOpenCodeExport(result.output, project: project)
            guard snapshot.sessionID == sessionID else {
                return unavailable(project: project, sessionID: sessionID, error: "OpenCode returned a different session than requested.")
            }
            if !project.isEmpty, !snapshot.project.isEmpty,
               canonicalPath(snapshot.project) != canonicalPath(project) {
                return unavailable(project: project, sessionID: sessionID,
                                   error: "The recorded session belongs to a different workspace. Check the selected project and worker log in Settings.")
            }
            let configuration = configuredRuntime()
            for record in configuration.records {
                if let index = snapshot.runtime.firstIndex(where: { $0.id == record.id }) { snapshot.runtime[index] = record }
                else { snapshot.runtime.append(record) }
            }
            snapshot.warnings.append(contentsOf: configuration.warnings)
            return snapshot
        } catch {
            return unavailable(project: project, sessionID: sessionID, error: error.localizedDescription)
        }
    }

    static func parseOpenCodeExport(_ json: String, project: String = "", collectedAt: Date = Date()) throws -> TelemetrySnapshot {
        guard json.utf8.count <= 32 * 1_024 * 1_024 else { throw ParseError.tooLarge }
        guard let data = json.data(using: .utf8), let parsed = try? JSONSerialization.jsonObject(with: data),
              var root = parsed as? [String: Any] else { throw ParseError.invalidJSON }
        if root["info"] == nil, let wrapped = root["data"] as? [String: Any] { root = wrapped }
        guard let info = root["info"] as? [String: Any], let id = info["id"] as? String,
              SessionsPaneData.isSessionID(id), let rawMessages = root["messages"] as? [[String: Any]] else {
            throw ParseError.unsupportedFormat
        }
        let location = info["location"] as? [String: Any]
        let recordedProject = string(location?["directory"]) ?? string(info["directory"])
        var snapshot = TelemetrySnapshot(sessionID: id, title: clean(string(info["title"]) ?? "Untitled session"),
                                         project: clean(recordedProject ?? project),
                                         collectedAt: collectedAt)
        snapshot.rawInfoJSON = pretty(info)
        if let recordedProject, !project.isEmpty, canonicalPath(recordedProject) != canonicalPath(project) {
            snapshot.warnings.append("The export’s recorded workspace differs from the requested project.")
        }
        snapshot.parentSessionID = string(info["parentID"] ?? info["parentSessionID"])
        snapshot.usage = usage(info["tokens"])
        snapshot.recordedCost = cost(info["cost"])
        snapshot.costCurrency = currency(info["cost"]) ?? string(info["currency"])
        let sessionModel = model(info["model"])
        snapshot.model = sessionModel.model
        snapshot.provider = sessionModel.provider
        var eventOrder: [(Int, TelemetryToolEvent, Date?)] = []
        var seenLinks = Set<String>()

        for (index, rawMessage) in rawMessages.enumerated() {
            let legacyInfo = rawMessage["info"] as? [String: Any]
            let message = legacyInfo ?? rawMessage
            let kind = string(message["type"] ?? message["role"]) ?? "unknown"
            let messageID = string(message["id"]) ?? "message-\(index)"
            let time = message["time"] as? [String: Any]
            let timestamp = date(time?["created"])
            let content = (rawMessage["content"] ?? rawMessage["parts"]) as? [[String: Any]] ?? []
            var text = string(message["text"]) ?? ""
            if kind == "assistant" {
                let recordedModel = model(message["model"])
                snapshot.model = sessionModel.model ?? recordedModel.model ?? string(message["modelID"]) ?? snapshot.model
                snapshot.provider = sessionModel.provider ?? recordedModel.provider ?? string(message["providerID"]) ?? snapshot.provider
                snapshot.agent = cleanOptional(string(message["agent"])) ?? snapshot.agent
            }
            if kind == "agent-switched" { snapshot.agent = cleanOptional(string(message["agent"])) ?? snapshot.agent }
            for (partIndex, part) in content.enumerated() {
                let partKind = string(part["type"]) ?? "unknown"
                if partKind == "text" || partKind == "reasoning" {
                    let partText = string(part["text"]) ?? ""
                    if !text.isEmpty && !partText.isEmpty { text += "\n\n" }
                    if partKind == "reasoning" {
                        text += "Recorded reasoning\n"
                        snapshot.context.append(record(id: "\(messageID)-reasoning-\(partIndex)", title: "Recorded reasoning", value: partText, detail: part))
                    }
                    text += partText
                } else if partKind == "tool" {
                    let state = part["state"] as? [String: Any] ?? [:]
                    let input = state["input"] ?? part["input"]
                    let output = state["content"] ?? state["output"] ?? part["output"]
                    let eventTime = part["time"] as? [String: Any] ?? state["time"] as? [String: Any] ?? [:]
                    let start = date(eventTime["ran"] ?? eventTime["start"] ?? eventTime["created"])
                    let end = date(eventTime["completed"] ?? eventTime["end"])
                    let duration = start.flatMap { start in end.flatMap { $0 >= start ? $0.timeIntervalSince(start) : nil } }
                    let eventID = string(part["id"] ?? part["callID"]) ?? "\(messageID)-tool-\(partIndex)"
                    let name = clean(string(part["name"] ?? part["tool"]) ?? "Unknown tool")
                    let event = TelemetryToolEvent(id: eventID, name: name, status: clean(string(state["status"] ?? part["status"]) ?? "Not recorded"),
                        inputJSON: input.map(pretty) ?? "Not recorded", output: output.map(renderContent),
                        error: state["error"].map(renderContent), startedAt: start, completedAt: end,
                        duration: duration, rawJSON: pretty(part))
                    eventOrder.append((eventOrder.count, event, start ?? timestamp))
                    if let subagent = subagent(event: event, part: part, input: input), seenLinks.insert(subagent.id).inserted {
                        snapshot.subagents.append(subagent)
                    }
                }
            }
            if kind == "user" {
                if !text.isEmpty { snapshot.taskPrompt = clean(text) }
                snapshot.context.append(record(id: messageID, title: "User prompt \(index + 1)", value: text, detail: rawMessage))
            } else if ["system", "synthetic", "skill"].contains(kind) {
                snapshot.context.append(record(id: messageID, title: "\(kind.capitalized) context", value: text, detail: rawMessage))
            }
            snapshot.messages.append(TelemetryMessage(id: messageID, type: clean(kind), text: clean(text),
                                                       timestamp: timestamp, rawJSON: pretty(rawMessage)))
        }
        snapshot.toolEvents = eventOrder.sorted { lhs, rhs in
            let left = lhs.2 ?? .distantPast
            let right = rhs.2 ?? .distantPast
            return left == right ? lhs.0 < rhs.0 : left < right
        }.map(\.1)
        snapshot.runtime = baseRuntime(project: snapshot.project)
        if let outcome = string(info["outcome"]) {
            snapshot.runtime.append(record(id: "outcome", title: "Session outcome", value: outcome, detail: ["outcome": outcome, "time": info["time"] ?? [:]]))
        }
        snapshot.context.append(record(id: "export-coverage", title: "Context coverage", value: "Recorded messages and supplied context", detail: [
            "coverage": "The export preserves recorded user, system, skill, synthetic, assistant and reasoning messages. It does not prove that the complete provider request or effective system prompt was retained.",
            "recordedMessageCount": snapshot.messages.count
        ]))
        if snapshot.usage == nil { snapshot.warnings.append("No aggregate token usage was recorded for this session.") }
        if snapshot.recordedCost == nil { snapshot.warnings.append("No aggregate cost was recorded for this session.") }
        if snapshot.recordedCost != nil && snapshot.costCurrency == nil {
            snapshot.warnings.append("The export records a numeric cost without a currency label.")
        }
        return snapshot
    }

    private static var executable: String { NSHomeDirectory() + "/.opencode/bin/opencode" }

    private static func canonicalPath(_ path: String) -> String {
        URL(fileURLWithPath: path).standardizedFileURL.resolvingSymlinksInPath().path
    }

    private static func unavailable(project: String, sessionID: String, error: String?) -> TelemetrySnapshot {
        var snapshot = TelemetrySnapshot(sessionID: sessionID, title: "Telemetry unavailable", project: project, collectedAt: Date())
        snapshot.error = error
        snapshot.runtime = baseRuntime(project: project)
        return snapshot
    }

    private static func commandFailure(_ code: Int32, operation: String) -> String {
        code == 124 ? "OpenCode timed out while trying to \(operation). Try Refresh."
            : "OpenCode could not \(operation) (exit \(code)). Try Refresh."
    }

    private static func usage(_ raw: Any?) -> TelemetryUsage? {
        guard let value = raw as? [String: Any] else { return nil }
        let cache = value["cache"] as? [String: Any] ?? [:]
        let result = TelemetryUsage(input: integer(value["input"]), output: integer(value["output"]),
            reasoning: integer(value["reasoning"]), cacheRead: integer(cache["read"] ?? value["cacheRead"]),
            cacheWrite: integer(cache["write"] ?? value["cacheWrite"]), total: integer(value["total"]))
        return [result.input, result.output, result.reasoning, result.cacheRead, result.cacheWrite, result.total].contains(where: { $0 != nil }) ? result : nil
    }

    private static func model(_ value: Any?) -> (model: String?, provider: String?) {
        if let object = value as? [String: Any] {
            return (cleanOptional(string(object["id"] ?? object["modelID"])), cleanOptional(string(object["providerID"] ?? object["provider"])))
        }
        return (cleanOptional(string(value)), nil)
    }

    private static func cost(_ value: Any?) -> Double? {
        if let object = value as? [String: Any] { return number(object["amount"] ?? object["value"]) }
        return number(value)
    }

    private static func currency(_ value: Any?) -> String? {
        guard let object = value as? [String: Any] else { return nil }
        return cleanOptional(string(object["currency"]))
    }

    private static func number(_ value: Any?) -> Double? {
        guard let value = value as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID(), value.doubleValue.isFinite else { return nil }
        return value.doubleValue
    }

    private static func integer(_ value: Any?) -> Int64? {
        guard let value = number(value), value >= 0, value < Double(Int64.max), value.rounded() == value else { return nil }
        return Int64(value)
    }

    private static func string(_ value: Any?) -> String? { value as? String }

    private static func date(_ value: Any?) -> Date? {
        if let value = number(value) { return Date(timeIntervalSince1970: abs(value) > 100_000_000_000 ? value / 1_000 : value) }
        if let value = value as? String {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            if let date = formatter.date(from: value) { return date }
            formatter.formatOptions = [.withInternetDateTime]
            return formatter.date(from: value)
        }
        return nil
    }

    private static func subagent(event: TelemetryToolEvent, part: [String: Any], input: Any?) -> TelemetrySubagent? {
        let args = input as? [String: Any] ?? [:]
        let state = part["state"] as? [String: Any] ?? [:]
        let metadata = state["metadata"] as? [String: Any] ?? [:]
        let names = ["sessionID", "sessionId", "session_id", "childSessionID", "childSessionId", "task_id"]
        var sessionID = names.compactMap { string(metadata[$0] ?? args[$0]) }.first(where: SessionsPaneData.isSessionID)
        let isTask = ["task", "agent", "subagent", "delegate_task", "spawn_agent"].contains(event.name.lowercased())
        guard isTask || sessionID != nil else { return nil }
        if sessionID == nil, let output = event.output {
            sessionID = capture(#"\b(ses_[A-Za-z0-9_-]+)\b"#, in: output)
        }
        return TelemetrySubagent(id: event.id, title: clean(string(args["description"] ?? args["title"]) ?? event.name),
            sessionID: sessionID, agent: cleanOptional(string(args["subagent_type"] ?? args["agent_type"] ?? args["agent"])),
            status: event.status, prompt: cleanOptional(string(args["prompt"] ?? args["message"] ?? args["task"])), rawJSON: event.rawJSON)
    }

    private static func baseRuntime(project: String) -> [TelemetryRecord] {
        [record(id: "harness", title: "Harness", value: "OpenCode", detail: ["source": "Installed OpenCode session export", "readOnly": true]),
         record(id: "workspace", title: "Workspace", value: project.isEmpty ? "Not recorded" : project, detail: ["project": project]),
         record(id: "permissions", title: "Permissions", value: "Effective approvals not reported", detail: ["coverage": "An exported transcript does not establish current permission rules or approval decisions."]),
         record(id: "plugins", title: "Plugins", value: "Effective plugins not reported", detail: ["coverage": "Only explicitly configured plugin identifiers can be shown; runtime loading is not established."]),
         record(id: "mcp", title: "MCP servers", value: "Effective servers not reported", detail: ["coverage": "Configuration is not a live connection check."]),
         record(id: "sandbox", title: "Sandbox", value: "Not reported by the export", detail: ["coverage": "No effective OS sandbox or isolation policy is inferred from tool names or workspace paths."])]
    }

    private static func configuredRuntime() -> (records: [TelemetryRecord], warnings: [String]) {
        var configurations: [[String: Any]] = []
        var paths: [String] = []
        var warnings: [String] = []
        for name in ["opencode.json", "opencode.jsonc"] {
            let path = NSHomeDirectory() + "/.config/opencode/" + name
            guard FileManager.default.fileExists(atPath: path) else { continue }
            do {
                let file = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
                defer { try? file.close() }
                let data = try file.read(upToCount: 1_024 * 1_024 + 1) ?? Data()
                guard data.count <= 1_024 * 1_024 else { warnings.append("\(name) exceeds the 1 MiB configuration read limit."); continue }
                guard let object = try JSONSerialization.jsonObject(with: Data(stripJSONComments(String(decoding: data, as: UTF8.self)).utf8)) as? [String: Any] else {
                    warnings.append("\(name) is not a configuration object."); continue
                }
                var safe: [String: Any] = [:]
                for key in ["model", "small_model", "default_agent"] { if let value = object[key] as? String { safe[key] = clean(value) } }
                if let permissions = object["permission"] { safe["permission"] = permissionRules(permissions) }
                if let plugins = object["plugin"] as? [String] { safe["plugin"] = plugins.map(clean) }
                if let instructions = object["instructions"] as? [String] { safe["instructions"] = instructions.map(clean) }
                if let mcp = object["mcp"] as? [String: Any] {
                    safe["mcp"] = mcp.mapValues { raw -> [String: Any] in
                        guard let server = raw as? [String: Any] else { return [:] }
                        var value: [String: Any] = [:]
                        if let type = server["type"] as? String { value["type"] = clean(type) }
                        if let enabled = server["enabled"] as? Bool { value["enabled"] = enabled }
                        if let url = server["url"] as? String { value["origin"] = URL(string: url).flatMap(SessionsPaneData.origin) ?? "Invalid URL" }
                        if let command = server["command"] as? [String], let executable = command.first { value["executable"] = clean(executable) }
                        return value
                    }
                }
                configurations.append(["file": path, "configured": safe])
                paths.append(path)
            } catch { warnings.append("\(name) could not be read as JSON or JSONC.") }
        }
        guard !configurations.isEmpty else { return ([], warnings) }
        var records = [record(id: "configuration", title: "Configuration", value: "Whitelisted global settings", detail: configurations)]
        let safeObjects = configurations.compactMap { $0["configured"] as? [String: Any] }
        for (key, id, title) in [("permission", "permissions", "Permissions"), ("plugin", "plugins", "Plugins"), ("mcp", "mcp", "MCP servers")] {
            let configured = safeObjects.compactMap { $0[key] }
            if !configured.isEmpty {
                records.append(record(id: id, title: title, value: "Configured globally; effective state not reported", detail: ["files": paths, "configured": configured]))
            }
        }
        warnings.append("Global configuration is shown through a whitelist. Project overrides, effective approvals, runtime plugin loading and OS sandbox enforcement are not established by this export.")
        return (records, warnings)
    }

    private static func permissionRules(_ raw: Any, depth: Int = 0) -> Any {
        guard depth < 12 else { return "[nested rule unavailable]" }
        if let value = raw as? String { return ["allow", "deny", "ask"].contains(value) ? value : "[unrecognized rule]" }
        if let value = raw as? [String: Any] {
            var rules: [String: Any] = [:]
            for (key, rule) in value { rules[clean(key)] = permissionRules(rule, depth: depth + 1) }
            return rules
        }
        if let value = raw as? Bool { return value }
        return "[unrecognized rule]"
    }

    // Strip comments/trailing commas without changing quoted URLs or prompts.
    static func stripJSONComments(_ text: String) -> String {
        let chars = Array(text)
        var result = ""
        var index = 0
        var quoted = false
        var escaped = false
        while index < chars.count {
            let char = chars[index]
            if quoted {
                result.append(char)
                if escaped { escaped = false }
                else if char == "\\" { escaped = true }
                else if char == "\"" { quoted = false }
                index += 1; continue
            }
            if char == "\"" { quoted = true; result.append(char); index += 1; continue }
            if char == "/", index + 1 < chars.count, chars[index + 1] == "/" {
                index += 2
                while index < chars.count && chars[index] != "\n" { index += 1 }
                continue
            }
            if char == "/", index + 1 < chars.count, chars[index + 1] == "*" {
                index += 2
                while index + 1 < chars.count && !(chars[index] == "*" && chars[index + 1] == "/") { index += 1 }
                index = min(chars.count, index + 2)
                result.append(" "); continue
            }
            if char == "," {
                var next = index + 1
                while next < chars.count && chars[next].isWhitespace { next += 1 }
                if next < chars.count && ["}", "]"].contains(chars[next]) { index += 1; continue }
            }
            result.append(char); index += 1
        }
        let stripped = Array(result)
        var trailing = ""
        quoted = false
        escaped = false
        for (offset, char) in stripped.enumerated() {
            if quoted {
                trailing.append(char)
                if escaped { escaped = false }
                else if char == "\\" { escaped = true }
                else if char == "\"" { quoted = false }
                continue
            }
            if char == "\"" { quoted = true }
            if char == "," {
                var next = offset + 1
                while next < stripped.count && stripped[next].isWhitespace { next += 1 }
                if next < stripped.count && ["}", "]"].contains(stripped[next]) { continue }
            }
            trailing.append(char)
        }
        return trailing
    }

    private static func record(id: String, title: String, value: String, detail: Any) -> TelemetryRecord {
        TelemetryRecord(id: id, title: title, value: String(clean(value).prefix(240)), detailJSON: pretty(detail))
    }

    private static func renderContent(_ value: Any) -> String {
        if let text = value as? String { return clean(text) }
        if let array = value as? [Any] { return array.map(renderContent).joined(separator: "\n\n") }
        if let object = value as? [String: Any], let text = object["text"] as? String { return clean(text) }
        return pretty(value)
    }

    private static func pretty(_ value: Any) -> String {
        let safe = redact(value)
        guard let data = try? JSONSerialization.data(withJSONObject: safe, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]),
              let text = String(data: data, encoding: .utf8) else { return clean(String(describing: safe)) }
        return text
    }

    private static func redact(_ value: Any) -> Any {
        if let object = value as? [String: Any] {
            var result: [String: Any] = [:]
            for (key, value) in object {
                let normalized = key.lowercased().filter { $0.isLetter || $0.isNumber }
                let sensitive = ["apikey", "accesskey", "accesstoken", "refreshtoken", "authorization", "password", "secret", "credential", "privatekey", "clientsecret", "cookie"].contains { normalized.contains($0) }
                    || (normalized.hasSuffix("token") && value is String)
                    || ["env", "environment"].contains(normalized)
                result[clean(key)] = sensitive ? "[redacted]" : redact(value)
            }
            return result
        }
        if let array = value as? [Any] { return array.map(redact) }
        if let text = value as? String { return clean(text) }
        return value
    }

    private static func cleanOptional(_ value: String?) -> String? { value.map(clean) }

    private static func clean(_ value: String) -> String {
        // Tool arguments can themselves be JSON stored inside a JSON string.
        // Decode that layer before matching assignments with escaped quotes.
        if let data = value.data(using: .utf8), let nested = try? JSONSerialization.jsonObject(with: data),
           nested is [String: Any] || nested is [Any],
           let safe = try? JSONSerialization.data(withJSONObject: redact(nested), options: [.sortedKeys, .withoutEscapingSlashes]),
           let text = String(data: safe, encoding: .utf8) { return text }
        var text = value
        for (pattern, replacement) in [
            (#"(?i)\b([A-Z_][A-Z0-9_]*(?:API_KEY|SECRET_ACCESS_KEY|ACCESS_KEY_ID|TOKEN|PASSWORD|SECRET|PRIVATE_KEY|CREDENTIALS?)|TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY_ID)[\"']?\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|[^\s\"'`,;]+)"#, "$1=[redacted]"),
            (#"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+"#, "Bearer [redacted]"),
            (#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|password|client[_-]?secret|authorization)[\"']?\s*[:=]\s*[\"']?[^\s\"'`,;]+"#, "$1=[redacted]"),
            (#"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"#, "[private key redacted]")
        ] { text = text.replacingOccurrences(of: pattern, with: replacement, options: .regularExpression) }
        if let regex = try? NSRegularExpression(pattern: #"https?://[^\s<>\"']+"#) {
            let mutable = NSMutableString(string: text)
            for match in regex.matches(in: text, range: NSRange(text.startIndex..<text.endIndex, in: text)).reversed() {
                let raw = mutable.substring(with: match.range)
                guard let url = URL(string: raw), let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { continue }
                let sensitiveQuery = components.queryItems?.contains { item in
                    let key = item.name.lowercased()
                    return ["token", "key", "secret", "password", "auth", "credential", "code"].contains(where: key.contains)
                } ?? false
                if sensitiveQuery || components.user != nil || components.password != nil || components.path.lowercased().contains("/pair") || components.fragment != nil {
                    mutable.replaceCharacters(in: match.range, with: SessionsPaneData.origin(for: url) ?? "[service URL redacted]")
                }
            }
            text = mutable as String
        }
        return text
    }
}
