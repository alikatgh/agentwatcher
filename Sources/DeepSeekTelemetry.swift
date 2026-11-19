import Foundation

// Reads the same append-only session records as DeepSeek Harness Desktop. The
// caller schedules filesystem work off the main thread; this adapter never
// starts Harness, loads a plugin, or changes runtime/configuration state.
enum DeepSeekTelemetry {
    struct SessionDescriptor: Equatable {
        var id: String
        var title: String
        var project: String
        var url: URL
        var updated: Date
        var sessionsRoot: URL? = nil
    }

    enum ParseError: LocalizedError {
        case tooLarge, invalidJSON, unsupportedFormat, unsafePath, decoderUnavailable, decodeFailed
        var errorDescription: String? {
            switch self {
            case .tooLarge: return "The DeepSeek Harness record exceeds the 32 MiB telemetry limit."
            case .invalidJSON: return "DeepSeek Harness contains an invalid session-event record."
            case .unsupportedFormat: return "DeepSeek Harness did not record a supported session header."
            case .unsafePath: return "The session path is outside its DeepSeek Harness session directory or contains a symbolic link."
            case .decoderUnavailable: return "A local zstd decoder is required to read DeepSeek Harness compressed sessions."
            case .decodeFailed: return "The DeepSeek Harness compressed session could not be decoded."
            }
        }
    }

    private static let byteLimit = 32 * 1_024 * 1_024
    private static let supportedFiles = ["session.v4.jsonl.zstd", "session.v4.jsonl", "session.jsonl"]

    static func sessions(homeURL: URL = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".dsh")) -> [SessionDescriptor] {
        let home = homeURL.standardizedFileURL.resolvingSymlinksInPath()
        let root = home.appendingPathComponent("sessions", isDirectory: true)
        guard safe(root, beneath: home), let projects = directories(root, limit: 128) else { return [] }
        var discovered: [SessionDescriptor] = []
        let cacheRoot = home.appendingPathComponent("storages/session_projcache/sessions", isDirectory: true)
        for project in projects {
            guard safe(project, beneath: root), let entries = directories(project, limit: 512) else { continue }
            for entry in entries where discovered.count < 512 {
                let id = entry.lastPathComponent
                guard isSessionID(id), safe(entry, beneath: root) else { continue }
                guard let file = supportedFiles.map({ entry.appendingPathComponent($0) }).first(where: {
                    safe($0, beneath: root) && regularFile($0)
                }) else { continue }
                let modified = (try? file.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
                var title = "Untitled session"
                var cwd = ""
                // The desktop projection cache supplies just list metadata. It
                // avoids decompressing every full conversation to show a list.
                let cache = cacheRoot.appendingPathComponent(id + ".json")
                if safe(cache, beneath: home), let data = try? read(cache),
                   let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                   let record = object["record"] as? [String: Any] {
                    let identity = record["identity"] as? [String: Any]
                    let rows = record["rows"] as? [String: Any]
                    cwd = string(identity?["cwd"]) ?? ""
                    if let row = rows?["title"] as? [String: Any], let value = string(row["val"]), !value.isEmpty { title = value }
                }
                // Uncompressed fixtures/older recordings can be listed without
                // a projection cache. Compressed files remain selectable even
                // when the optional desktop cache has not been written yet.
                if cwd.isEmpty, file.pathExtension != "zstd", let data = try? read(file),
                   let text = String(data: data, encoding: .utf8) {
                    for line in text.split(separator: "\n") {
                        guard let object = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else { continue }
                        if string(object["type"]) == "session" { cwd = string(object["cwd"]) ?? cwd }
                        if string(object["type"]) == "session/title", let payload = object["data"] as? [String: Any] {
                            title = string(payload["title"]) ?? title
                        }
                    }
                }
                discovered.append(SessionDescriptor(id: id, title: clean(title), project: clean(cwd), url: file,
                                                    updated: modified, sessionsRoot: root))
            }
        }
        return discovered.sorted { $0.updated == $1.updated ? $0.id < $1.id : $0.updated > $1.updated }
    }

    static func collect(session: SessionDescriptor) -> TelemetrySnapshot {
        do {
            let root = session.sessionsRoot ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".dsh/sessions")
            guard supportedFiles.contains(session.url.lastPathComponent), isSessionID(session.id),
                  session.url.deletingLastPathComponent().lastPathComponent == session.id,
                  safe(session.url, beneath: root), regularFile(session.url) else { throw ParseError.unsafePath }
            let text: String
            var decodingWarning: String?
            if session.url.pathExtension == "zstd" {
                _ = try read(session.url) // Bound compressed input before invoking a decoder.
                guard let decoder = ["/opt/homebrew/bin/zstd", "/usr/local/bin/zstd", "/usr/bin/zstd"].first(where: {
                    FileManager.default.isExecutableFile(atPath: $0)
                }) else { throw ParseError.decoderUnavailable }
                let result = run(decoder, ["--decompress", "--stdout", "--quiet", "--", session.url.path], timeout: 10)
                // A live writer may leave the final zstd frame incomplete. Only
                // complete JSON rows are parsed; stderr is removed explicitly.
                if result.code != 0 {
                    let unfinishedFrame = ["premature end", "unexpected end", "did not complete", "incomplete frame"].contains {
                        result.output.lowercased().contains($0)
                    }
                    guard unfinishedFrame, result.code != 124, !result.output.contains("32 MiB capture limit"),
                          let finalNewline = result.output.lastIndex(of: "\n") else { throw ParseError.decodeFailed }
                    let lines = result.output[..<finalNewline].split(separator: "\n", omittingEmptySubsequences: false)
                    let validPrefix = lines.prefix { line in
                        line.trimmingCharacters(in: .whitespaces).isEmpty || (try? JSONSerialization.jsonObject(with: Data(line.utf8))) != nil
                    }
                    guard !validPrefix.isEmpty else { throw ParseError.decodeFailed }
                    text = validPrefix.joined(separator: "\n") + "\n"
                    decodingWarning = "The live compressed record ended in an incomplete frame. Showing its complete recorded events."
                } else { text = result.output }
            } else {
                guard let value = String(data: try read(session.url), encoding: .utf8) else { throw ParseError.invalidJSON }
                text = value
            }
            var snapshot = try parseSessionEvents(text, session: session)
            if let decodingWarning { snapshot.warnings.append(decodingWarning) }
            return snapshot
        } catch {
            var snapshot = TelemetrySnapshot(sessionID: session.id, title: session.title, project: session.project, collectedAt: Date())
            snapshot.source = "DeepSeek Harness session events"
            snapshot.error = error.localizedDescription
            return snapshot
        }
    }

    static func parseSessionEvents(_ jsonl: String, session: SessionDescriptor, collectedAt: Date = Date()) throws -> TelemetrySnapshot {
        guard jsonl.utf8.count <= byteLimit else { throw ParseError.tooLarge }
        let lines = jsonl.split(separator: "\n", omittingEmptySubsequences: false)
        var events: [[String: Any]] = []
        var partial = false
        for (index, line) in lines.enumerated() where !line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard let object = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else {
                if index == lines.count - 1 && !jsonl.hasSuffix("\n") { partial = true; break }
                throw ParseError.invalidJSON
            }
            events.append(object)
        }
        guard let header = events.first, string(header["type"]) == "session", integer(header["version"]) == 4, let id = string(header["id"]),
              isSessionID(id), session.id.isEmpty || session.id == id else { throw ParseError.unsupportedFormat }
        var snapshot = TelemetrySnapshot(sessionID: id, title: session.title,
                                         project: clean(string(header["cwd"]) ?? session.project), collectedAt: collectedAt)
        snapshot.source = "DeepSeek Harness session events"
        snapshot.rawInfoJSON = pretty(header)
        snapshot.agent = string(header["agentPreset"]).map(clean)
        snapshot.parentSessionID = string(header["parentSessionId"] ?? header["parentSessionID"])
        var turn: Int?
        var step: Int?
        var schemas: [String: [String: Any]] = [:]
        var tools: [TelemetryToolEvent] = []
        var toolDetails: [[String: Any]] = []
        var indices: [String: Int] = [:]
        var usages: [String: TelemetryUsage] = [:]
        var subagents: [String: TelemetrySubagent] = [:]
        var subagentOrder: [String] = []
        var costs: [String: Double] = [:]
        var costCurrencies: [String: String] = [:]
        var latestHumanPrompt: String?
        var firstUnclassifiedPrompt: String?

        for (index, event) in events.enumerated() {
            let type = string(event["type"]) ?? "unknown"
            let data = event["data"] as? [String: Any] ?? [:]
            let timestamp = date(event["time"] ?? event["createdAt"])
            let eventID = "event-\(string(event["seq"]) ?? String(index))"
            if let value = integer(data["turn"]) { turn = value }
            if let value = integer(data["step"]) { step = value }

            switch type {
            case "session":
                snapshot.runtime.append(record("session-header", "Session identity", "Format \(string(event["version"]) ?? "unknown")", event))
            case "session/title":
                snapshot.title = clean(string(data["title"]) ?? snapshot.title)
            case "model/selection":
                snapshot.model = string(data["model"]).map(clean) ?? snapshot.model
                snapshot.provider = string(data["provider"]).map(clean) ?? snapshot.provider
                upsert(record("model-selection", "Model selection", display(data), event), in: &snapshot.runtime)
            case "agent-preset/selected":
                snapshot.agent = string(data["agentPreset"]).map(clean) ?? snapshot.agent
                upsert(record("agent-preset", "Agent preset", snapshot.agent ?? "Not recorded", event), in: &snapshot.runtime)
            case "request/header":
                let request = data["header"] as? [String: Any] ?? [:]
                let config = request["config"] as? [String: Any] ?? [:]
                snapshot.model = string(config["model"]).map(clean) ?? snapshot.model
                snapshot.provider = string(config["provider"]).map(clean) ?? snapshot.provider
                for tool in request["tools"] as? [[String: Any]] ?? [] {
                    if let name = string(tool["name"]) { schemas[name] = tool }
                }
                snapshot.context.append(record(eventID, "Provider request header", "\(schemas.count) recorded tool schemas", event))
            case "request/context":
                snapshot.model = string(data["model"]).map(clean) ?? snapshot.model
                snapshot.provider = string(data["provider"]).map(clean) ?? snapshot.provider
                snapshot.context.append(record(eventID, "Provider request context", display(data), event))
            case "system/message", "developer/message", "user/message", "assistant/message":
                let message = data["message"] as? [String: Any] ?? data
                let messageID = string(message["id"]) ?? eventID
                let role = string(message["role"]) ?? String(type.prefix { $0 != "/" })
                let text = render(message["content"] ?? message["text"] ?? "")
                snapshot.messages.append(TelemetryMessage(id: messageID + "-\(index)", type: clean(role), text: text,
                                                         timestamp: timestamp, rawJSON: pretty(event)))
                if role == "user" {
                    let source = message["source"] as? [String: Any]
                    let sourceKind = string(source?["kind"])
                    // DSH persists runtime-context snapshots with role=user.
                    // The source discriminant identifies actual human input;
                    // role alone must not let a later injection replace it.
                    if sourceKind == "user" { latestHumanPrompt = text }
                    else if sourceKind == nil && firstUnclassifiedPrompt == nil { firstUnclassifiedPrompt = text }
                    let title = sourceKind == nil || sourceKind == "user" ? "User prompt" : "\(sourceKind!) injection"
                    snapshot.context.append(record(eventID, title, text, event))
                } else if role == "system" || role == "developer" {
                    snapshot.context.append(record(eventID, role.capitalized + " prompt", text, event))
                }
                if role == "assistant" {
                    let source = message["source"] as? [String: Any] ?? [:]
                    snapshot.model = string(source["model"]).map(clean) ?? snapshot.model
                    snapshot.provider = string(source["provider"]).map(clean) ?? snapshot.provider
                    if let usage = usage(data["usage"]) { usages[messageID] = usage }
                    if let value = recordedCost(data["cost"]) {
                        costs[messageID] = value
                        costCurrencies[messageID] = currency(data["cost"]) ?? currency(data["currency"])
                    }
                    for (blockIndex, block) in (message["content"] as? [[String: Any]] ?? []).enumerated()
                        where string(block["type"]) == "reasoning" {
                        snapshot.context.append(record("\(eventID)-reasoning-\(blockIndex)", "Recorded reasoning", render(block), block))
                    }
                }
            case "tool/call":
                let callID = string(data["callId"] ?? data["id"]) ?? eventID
                let name = string(data["name"]) ?? "Unknown tool"
                let arguments = arguments(data["arguments"] ?? data["input"])
                var details: [String: Any] = ["call": event]
                if let turn { details["turn"] = turn }
                if let step { details["step"] = step }
                if let agent = snapshot.agent { details["agent"] = agent }
                if let schema = schemas[name] { details["toolSchema"] = schema }
                let tool = TelemetryToolEvent(id: callID, name: clean(name), status: "Result not recorded", inputJSON: pretty(arguments),
                    output: nil, error: nil, startedAt: timestamp, completedAt: nil, duration: nil, rawJSON: pretty(details))
                indices[callID] = tools.count
                tools.append(tool)
                toolDetails.append(details)
                if isSubagentTool(name), let agent = subagent(callID: callID, name: name, input: arguments, result: nil, raw: details) {
                    subagents[callID] = agent; subagentOrder.append(callID)
                }
            case "tool/result":
                let message = data["message"] as? [String: Any] ?? data
                let source = message["source"] as? [String: Any] ?? [:]
                guard let callID = string(message["toolCallId"] ?? source["callId"] ?? data["callId"]) else {
                    snapshot.warnings.append("A recorded tool result has no call ID."); continue
                }
                let output = render(message["content"] ?? message["output"] ?? "")
                let isError = message["isError"] as? Bool ?? false
                let toolIndex: Int
                if let existing = indices[callID] { toolIndex = existing }
                else {
                    toolIndex = tools.count; indices[callID] = toolIndex
                    tools.append(TelemetryToolEvent(id: callID, name: clean(string(data["name"]) ?? "Unknown tool"), status: "Call not recorded",
                        inputJSON: "Not recorded", output: nil, error: nil, startedAt: nil, completedAt: nil, duration: nil, rawJSON: "{}"))
                    toolDetails.append([:])
                    snapshot.warnings.append("A tool result is recorded without its corresponding call: \(clean(callID)).")
                }
                tools[toolIndex].status = isError ? "Error" : "Completed"
                tools[toolIndex].output = output
                tools[toolIndex].error = isError ? output : nil
                tools[toolIndex].completedAt = timestamp
                tools[toolIndex].duration = tools[toolIndex].startedAt.flatMap { start in timestamp.flatMap { $0 >= start ? $0.timeIntervalSince(start) : nil } }
                toolDetails[toolIndex]["result"] = event
                let linkedChild = linkedSession(message) ?? (isSubagentTool(tools[toolIndex].name) ? renderedSubagentLink(output) : nil)
                if let child = linkedChild { toolDetails[toolIndex]["subagentSessionID"] = child }
                tools[toolIndex].rawJSON = pretty(toolDetails[toolIndex])
                if isSubagentTool(tools[toolIndex].name), let agent = subagent(callID: callID, name: tools[toolIndex].name,
                    input: argumentsFromJSON(tools[toolIndex].inputJSON), result: message, raw: toolDetails[toolIndex]) {
                    if subagents[callID] == nil { subagentOrder.append(callID) }
                    subagents[callID] = agent
                }
            case "turn/start", "step/start", "step/end", "turn/end":
                snapshot.runtime.append(record(eventID, type.replacingOccurrences(of: "/", with: " ").capitalized, display(data), event))
            default:
                if type.hasPrefix("agent/inbox") || type.contains("injection") || type.hasPrefix("skill/") || type.contains("context") {
                    snapshot.context.append(record(eventID, clean(type), display(data), event))
                } else {
                    // Keep recorded permission, sandbox, approval, plugin, goal,
                    // background-job and future event fields inspectable.
                    snapshot.runtime.append(record(eventID, clean(type), display(data), event))
                }
                if type.hasPrefix("subagent/"), let childID = linkedSession(data) {
                    let key = string(data["agentId"] ?? data["id"]) ?? childID
                    let previous = subagents[key]
                    if previous == nil { subagentOrder.append(key) }
                    subagents[key] = TelemetrySubagent(id: key, title: clean(string(data["title"] ?? data["name"] ?? data["label"]) ?? previous?.title ?? "Recorded subagent"),
                        sessionID: childID, agent: string(data["agentType"] ?? data["agentPreset"]).map(clean) ?? previous?.agent,
                        status: string(data["status"] ?? data["stopReason"]).map(clean) ?? previous?.status ?? (type == "subagent/catalog" ? "Recorded child" : nil), prompt: string(data["prompt"]).map(clean) ?? previous?.prompt,
                        rawJSON: pretty(event))
                }
            }
        }
        snapshot.taskPrompt = latestHumanPrompt ?? firstUnclassifiedPrompt
        snapshot.toolEvents = tools
        for var child in subagentOrder.compactMap({ subagents[$0] }) {
            if let sessionID = child.sessionID, let existing = snapshot.subagents.firstIndex(where: { $0.sessionID == sessionID }) {
                let previous = snapshot.subagents[existing]
                child.id = previous.id
                child.prompt = child.prompt ?? previous.prompt
                child.agent = child.agent ?? previous.agent
                if child.title == "Recorded subagent" { child.title = previous.title }
                if child.status == nil || child.status == "Recorded child" { child.status = previous.status ?? child.status }
                child.rawJSON = pretty(["recordedLinks": [argumentsFromJSON(previous.rawJSON), argumentsFromJSON(child.rawJSON)]])
                snapshot.subagents[existing] = child
            } else { snapshot.subagents.append(child) }
        }
        snapshot.usage = summed(Array(usages.values))
        let currencies = Set(costCurrencies.values)
        if !costs.isEmpty, currencies.count <= 1 { snapshot.recordedCost = costs.values.reduce(0, +) }
        if currencies.count == 1 && costCurrencies.count == costs.count { snapshot.costCurrency = currencies.first }
        if snapshot.title.isEmpty || snapshot.title == "Untitled session" {
            snapshot.title = snapshot.taskPrompt.map { String($0.prefix(100)) } ?? "Untitled session"
        }
        snapshot.runtime.insert(record("record-source", "Recorded source", session.url.path, ["path": session.url.path, "format": "append-only DeepSeek Harness SessionEvents"]), at: 0)
        snapshot.context.append(record("context-coverage", "Context coverage", "Recorded prompts, request headers, context updates and injected messages", [
            "coverage": "Only retained session events are displayed. Request-header tool schemas and request-context updates are recorded evidence; they do not prove the complete effective provider request was retained.",
            "eventCount": events.count
        ]))
        if partial { snapshot.warnings.append("The final live JSONL record is incomplete. Showing complete recorded events.") }
        if snapshot.usage == nil { snapshot.warnings.append("No assistant token usage was recorded for this session.") }
        else { snapshot.runtime.append(record("usage-source", "Token usage source", "Sum of recorded assistant-message usage", ["messageCount": usages.count])) }
        if currencies.count > 1 { snapshot.warnings.append("Recorded costs use different currencies and cannot be added into a single total.") }
        else if snapshot.recordedCost == nil { snapshot.warnings.append("No cost was recorded. Token usage is shown without an estimated price.") }
        if snapshot.recordedCost != nil && snapshot.costCurrency == nil { snapshot.warnings.append("Recorded cost has no single explicit currency label.") }
        return snapshot
    }

    private static func safe(_ url: URL, beneath root: URL) -> Bool {
        let base = root.standardizedFileURL
        let path = url.standardizedFileURL
        guard path.path == base.path || path.path.hasPrefix(base.path + "/"),
              base.resolvingSymlinksInPath().path == base.path,
              path.resolvingSymlinksInPath().path == path.path else { return false }
        return true
    }

    private static func directories(_ root: URL, limit: Int) -> [URL]? {
        guard let enumeration = FileManager.default.enumerator(at: root,
            includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants]) else { return nil }
        var entries: [URL] = []
        var inspected = 0
        for case let url as URL in enumeration {
            inspected += 1
            if inspected > max(limit * 2, 256) { break }
            entries.append(url)
        }
        return entries.filter {
            guard let value = try? $0.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]) else { return false }
            return value.isDirectory == true && value.isSymbolicLink != true
        }.sorted {
            ((try? $0.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast) >
            ((try? $1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast)
        }.prefix(limit).map { $0 }
    }

    private static func regularFile(_ url: URL) -> Bool {
        guard let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]) else { return false }
        return values.isRegularFile == true && values.isSymbolicLink != true
    }

    private static func read(_ url: URL) throws -> Data {
        let values = try url.resourceValues(forKeys: [.fileSizeKey])
        guard let size = values.fileSize, size <= byteLimit else { throw ParseError.tooLarge }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let data = try handle.read(upToCount: byteLimit + 1) ?? Data()
        guard data.count <= byteLimit else { throw ParseError.tooLarge }
        return data
    }

    private static func isSessionID(_ id: String) -> Bool {
        id.range(of: #"^session-[A-Za-z0-9_-]+$"#, options: .regularExpression) != nil && id.count <= 160
    }

    private static func string(_ value: Any?) -> String? {
        if let value = value as? String { return value }
        if let value = value as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID() { return value.stringValue }
        return nil
    }
    private static func integer(_ value: Any?) -> Int? { string(value).flatMap(Int.init) }
    private static func date(_ value: Any?) -> Date? {
        if let number = value as? NSNumber { return Date(timeIntervalSince1970: number.doubleValue > 1e11 ? number.doubleValue / 1_000 : number.doubleValue) }
        guard let value = value as? String else { return nil }
        let parser = ISO8601DateFormatter(); parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return parser.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    }
    private static func clean(_ value: String) -> String { AgentTelemetry.redactedText(value) }
    private static func pretty(_ value: Any) -> String { AgentTelemetry.redactedJSON(value) }
    private static func record(_ id: String, _ title: String, _ value: String, _ raw: Any) -> TelemetryRecord {
        TelemetryRecord(id: id, title: clean(title), value: String(clean(value).prefix(240)), detailJSON: pretty(raw))
    }
    private static func upsert(_ value: TelemetryRecord, in array: inout [TelemetryRecord]) {
        if let index = array.firstIndex(where: { $0.id == value.id }) { array[index] = value } else { array.append(value) }
    }
    private static func display(_ value: [String: Any]) -> String {
        let safeKeys = ["title", "status", "preset", "mode", "policy", "model", "provider", "reasoningEffort", "contextWindow", "target", "turn", "step"]
        let parts = safeKeys.compactMap { key in string(value[key]).map { "\(key): \($0)" } }
        return parts.isEmpty ? "Recorded event · inspect details" : clean(parts.joined(separator: " · "))
    }
    private static func render(_ value: Any) -> String {
        if let text = value as? String { return clean(text) }
        if let array = value as? [Any] { return array.map(render).filter { !$0.isEmpty }.joined(separator: "\n\n") }
        if let object = value as? [String: Any] {
            if let text = string(object["text"]) { return clean(text) }
            if let type = string(object["type"]), type == "tool-call" { return "Tool call: \(clean(string(object["name"]) ?? "tool"))" }
        }
        return pretty(value)
    }
    private static func arguments(_ value: Any?) -> Any {
        if let text = value as? String { return argumentsFromJSON(text) }
        return value ?? "Not recorded"
    }
    private static func argumentsFromJSON(_ text: String) -> Any {
        (try? JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed])) ?? text
    }
    private static func usage(_ value: Any?) -> TelemetryUsage? {
        guard let object = value as? [String: Any] else { return nil }
        func number(_ keys: [String]) -> Int64? {
            for key in keys { if let text = string(object[key]), let value = Int64(text), value >= 0 { return value } }
            return nil
        }
        let result = TelemetryUsage(input: number(["inputTokens", "input_tokens"]), output: number(["outputTokens", "output_tokens"]),
            reasoning: number(["reasoningTokens", "reasoning_tokens"]), cacheRead: number(["cacheReadTokens", "cache_read_input_tokens"]),
            cacheWrite: number(["cacheWriteTokens", "cache_creation_input_tokens"]), total: number(["totalTokens", "total_tokens"]))
        return [result.input, result.output, result.reasoning, result.cacheRead, result.cacheWrite, result.total].contains { $0 != nil } ? result : nil
    }
    private static func summed(_ values: [TelemetryUsage]) -> TelemetryUsage? {
        guard !values.isEmpty else { return nil }
        func total(_ path: KeyPath<TelemetryUsage, Int64?>) -> Int64? {
            let recorded = values.compactMap { $0[keyPath: path] }
            guard !recorded.isEmpty else { return nil }
            var sum: Int64 = 0
            for value in recorded { let next = sum.addingReportingOverflow(value); guard !next.overflow else { return nil }; sum = next.partialValue }
            return sum
        }
        return TelemetryUsage(input: total(\.input), output: total(\.output), reasoning: total(\.reasoning), cacheRead: total(\.cacheRead), cacheWrite: total(\.cacheWrite), total: total(\.total))
    }
    private static func recordedCost(_ value: Any?) -> Double? {
        let object = value as? [String: Any]
        guard let number = (object?["amount"] ?? object?["total"] ?? value) as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite, number.doubleValue >= 0 else { return nil }
        return number.doubleValue
    }
    private static func currency(_ value: Any?) -> String? {
        if let object = value as? [String: Any] { return string(object["currency"]).map(clean) }
        if let text = value as? String { return clean(text) }
        return nil
    }
    private static func isSubagentTool(_ name: String) -> Bool { ["subagent", "subagent_fork", "spawn_agent"].contains(name) }
    private static func renderedSubagentLink(_ value: String) -> String? {
        // Exact renderer in the installed dsh-tool-subagent package. Arbitrary
        // prose containing session IDs does not establish a child relationship.
        let prefix = "started subagent "
        guard value.hasPrefix(prefix) else { return nil }
        let id = String(value.dropFirst(prefix.count)).trimmingCharacters(in: .whitespacesAndNewlines)
        return isSessionID(id) ? id : nil
    }
    private static func linkedSession(_ value: Any) -> String? {
        if let object = value as? [String: Any] {
            for key in ["sessionId", "sessionID", "childId", "childSessionId", "childSessionID", "subagentId", "subagentSessionId", "subagentSessionID"] {
                if let id = string(object[key]), isSessionID(id) { return id }
            }
            // Result payloads are often JSON text. Parse structured fields only;
            // a session-looking string in arbitrary output is not a proven link.
            for key in ["content", "result", "output", "data"] {
                if let nested = object[key], let id = linkedSession(nested) { return id }
            }
            if let text = object["text"] as? String, let parsed = try? JSONSerialization.jsonObject(with: Data(text.utf8)), let id = linkedSession(parsed) { return id }
        } else if let array = value as? [Any] {
            for nested in array { if let id = linkedSession(nested) { return id } }
        }
        return nil
    }
    private static func subagent(callID: String, name: String, input: Any, result: [String: Any]?, raw: [String: Any]) -> TelemetrySubagent? {
        let object = input as? [String: Any] ?? [:]
        let sessionID = result.flatMap(linkedSession) ?? result.flatMap { renderedSubagentLink(render($0["content"] ?? "")) } ?? linkedSession(object)
        let status = result?["isError"] as? Bool == true ? "Error" : (result == nil ? "Result not recorded" : "Spawn result recorded")
        return TelemetrySubagent(id: callID, title: clean(string(object["description"] ?? object["title"] ?? object["name"]) ?? name),
            sessionID: sessionID, agent: string(object["agentType"] ?? object["agentPreset"] ?? object["subagent_type"]).map(clean),
            status: status, prompt: string(object["prompt"] ?? object["task"]).map(clean), rawJSON: pretty(raw))
    }
}
