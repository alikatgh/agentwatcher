import Foundation
import CoreFoundation

// Reads append-only Codex rollout files. No daemon, database, credentials,
// model request, or runtime/configuration mutation is involved.
enum CodexTelemetry {
    struct SessionDescriptor: Equatable {
        var id: String
        var title: String
        var project: String
        var url: URL
        var updated: Date
        var parentSessionID: String? = nil
        var agent: String? = nil
        var sessionsRoot: URL? = nil
        var catalogNotice: String? = nil
    }

    private static let byteLimit = 32 * 1_024 * 1_024
    private static let headerLimit = 64 * 1_024
    private static let catalogLimit = 512
    private static let catalogDays = 45

    static func sessions(project: String, homeURL: URL = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".codex")) -> [SessionDescriptor] {
        guard !project.isEmpty else { return [] }
        let home = homeURL.standardizedFileURL
        let root = home.appendingPathComponent("sessions", isDirectory: true)
        guard safe(root, beneath: home) else { return [] }
        var found: [SessionDescriptor] = []
        var inspected = 0
        var truncated = false
        var seen = Set<String>()
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        let formatter = DateFormatter()
        formatter.calendar = calendar; formatter.timeZone = calendar.timeZone
        formatter.locale = Locale(identifier: "en_US_POSIX"); formatter.dateFormat = "yyyy/MM/dd"
        // Date-addressed lookup avoids recursively walking the user's Codex home.
        // Tomorrow allows a small clock/time-zone skew in live recordings.
        for offset in -1..<catalogDays {
            guard inspected < catalogLimit else { truncated = true; break }
            guard let day = calendar.date(byAdding: .day, value: -offset, to: Date()) else { continue }
            let directory = root.appendingPathComponent(formatter.string(from: day), isDirectory: true)
            guard safe(directory, beneath: root), let enumerator = FileManager.default.enumerator(at: directory,
                includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey, .contentModificationDateKey],
                options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants]) else { continue }
            var candidates: [URL] = []
            var directoryEntries = 0
            for case let file as URL in enumerator {
                directoryEntries += 1
                if directoryEntries > catalogLimit * 2 { truncated = true; break }
                if file.lastPathComponent.hasPrefix("rollout-"), file.pathExtension == "jsonl", safe(file, beneath: root), regular(file) {
                    candidates.append(file)
                }
            }
            candidates.sort { modified($0) > modified($1) }
            for file in candidates {
                guard inspected < catalogLimit else { truncated = true; break }
                inspected += 1
                guard let data = try? prefix(file, count: headerLimit),
                      let firstLine = data.split(separator: 10, maxSplits: 1).first,
                      let header = object(Data(firstLine)), string(header["type"]) == "session_meta",
                      let payload = header["payload"] as? [String: Any],
                      let id = string(payload["id"] ?? payload["session_id"]), isSessionID(id),
                      file.lastPathComponent.hasSuffix(id + ".jsonl"),
                      string(payload["cwd"]).map(normalized) == normalized(project),
                      !internalSession(payload), seen.insert(id).inserted else { continue }
                let spawn = spawnMetadata(payload)
                let parent = string(spawn["parent_thread_id"] ?? payload["parent_thread_id"]).flatMap { isSessionID($0) ? $0 : nil }
                let nickname = string(spawn["agent_nickname"] ?? payload["agent_nickname"])
                let role = string(spawn["agent_role"] ?? payload["agent_role"])
                let agentPath = string(spawn["agent_path"] ?? payload["agent_path"])
                let agent = [nickname, role, agentPath].compactMap { $0 }.joined(separator: " · ")
                var title = string(payload["title"])
                // Only this bounded prefix is inspected for a useful list label.
                // Injected AGENTS/environment instructions do not become tasks.
                if title == nil, let text = String(data: data, encoding: .utf8) {
                    for line in text.split(separator: "\n") {
                        guard let event = object(Data(line.utf8)), string(event["type"]) == "response_item",
                              let message = event["payload"] as? [String: Any], string(message["type"]) == "message",
                              string(message["role"]) == "user" else { continue }
                        let text = render(message["content"] ?? "")
                        if isHumanPrompt(text) { title = String(text.prefix(100)); break }
                    }
                }
                found.append(SessionDescriptor(id: id, title: clean(title ?? (agent.isEmpty ? "Codex session \(id.prefix(8))" : agent)),
                    project: project, url: file, updated: modified(file), parentSessionID: parent,
                    agent: agent.isEmpty ? nil : clean(agent), sessionsRoot: root))
            }
        }
        let notice = "The local Codex catalog covers the last \(catalogDays) UTC days and at most \(catalogLimit) rollout headers. Older sessions may be absent."
            + (truncated ? " The discovery limit was reached; additional recent sessions may be absent." : "")
        for index in found.indices { found[index].catalogNotice = notice }
        return found.sorted { $0.updated == $1.updated ? $0.id < $1.id : $0.updated > $1.updated }
    }

    static func collect(session: SessionDescriptor, sessions: [SessionDescriptor] = []) -> TelemetrySnapshot {
        let root = session.sessionsRoot ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".codex/sessions")
        guard isSessionID(session.id), session.url.lastPathComponent.hasPrefix("rollout-"),
              session.url.lastPathComponent.hasSuffix(session.id + ".jsonl"),
              safe(session.url, beneath: root), regular(session.url) else {
            return unavailable(session, "The Codex rollout is outside its sessions directory or contains a symbolic link.")
        }
        do {
            let size = try session.url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            let data: Data
            var coverageWarning: String?
            if size > byteLimit {
                let handle = try FileHandle(forReadingFrom: session.url)
                defer { try? handle.close() }
                let header = try handle.read(upToCount: headerLimit) ?? Data()
                guard let newline = header.firstIndex(of: 10) else {
                    return unavailable(session, "The Codex session header exceeds the 64 KiB header limit.")
                }
                // Keep the identity plus recent work. Total bytes read remain
                // bounded; the omitted middle is explicitly reported.
                let tailBudget = byteLimit - header.count
                try handle.seek(toOffset: UInt64(size - tailBudget))
                let tail = try handle.read(upToCount: tailBudget) ?? Data()
                let completeTail = tail.firstIndex(of: 10).map { tail.suffix(from: tail.index(after: $0)) } ?? Data.SubSequence()
                data = Data(header[...newline]) + Data(completeTail)
                coverageWarning = "This rollout exceeds 32 MiB. Showing its session header and the most recent complete rows within the 32 MiB read limit; earlier messages, calls and links may be absent."
            } else {
                data = try prefix(session.url, count: byteLimit)
            }
            var text = String(data: data, encoding: .utf8)
            var utf8Warning: String?
            if text == nil, let lastNewline = data.lastIndex(of: 10) {
                text = String(data: data[...lastNewline], encoding: .utf8)
                if text != nil { utf8Warning = "The final live UTF-8 record is incomplete. Showing preceding complete recorded events." }
            }
            guard let text else { return unavailable(session, "The Codex rollout is not valid UTF-8.") }
            var snapshot = parseSessionEvents(text, session: session, sessions: sessions)
            if let coverageWarning { snapshot.warnings.append(coverageWarning) }
            if let utf8Warning { snapshot.warnings.append(utf8Warning) }
            return snapshot
        } catch {
            return unavailable(session, "The Codex rollout could not be read: \(clean(error.localizedDescription))")
        }
    }

    static func parseSessionEvents(_ jsonl: String, session: SessionDescriptor, sessions: [SessionDescriptor] = [], collectedAt: Date = Date()) -> TelemetrySnapshot {
        guard jsonl.utf8.count <= byteLimit else { return unavailable(session, "The Codex rollout exceeds the 32 MiB telemetry limit.", collectedAt) }
        let lines = jsonl.split(separator: "\n", omittingEmptySubsequences: false)
        var events: [[String: Any]] = []
        var partial = false
        var malformed: Int?
        for (index, line) in lines.enumerated() where !line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard let value = object(Data(line.utf8)) else {
                if index == lines.count - 1 && !jsonl.hasSuffix("\n") { partial = true }
                else { malformed = index + 1 }
                break
            }
            events.append(value)
        }
        guard let header = events.first, string(header["type"]) == "session_meta",
              let info = header["payload"] as? [String: Any], let id = string(info["id"] ?? info["session_id"]),
              isSessionID(id), id == session.id, !internalSession(info) else {
            return unavailable(session, "The rollout has no supported matching Codex session header.", collectedAt)
        }
        guard let cwd = string(info["cwd"]), normalized(cwd) == normalized(session.project) else {
            return unavailable(session, "The Codex rollout belongs to a different workspace.", collectedAt)
        }
        var snapshot = TelemetrySnapshot(sessionID: id, title: clean(session.title), project: clean(cwd), collectedAt: collectedAt)
        snapshot.source = "Codex local rollout"
        snapshot.rawInfoJSON = pretty(header)
        let spawn = spawnMetadata(info)
        snapshot.parentSessionID = (string(spawn["parent_thread_id"] ?? info["parent_thread_id"]) ?? session.parentSessionID).flatMap { isSessionID($0) ? $0 : nil }
        snapshot.agent = (session.agent ?? string(spawn["agent_path"] ?? info["agent_path"])).map(clean)
        snapshot.provider = string(info["model_provider"]).map(clean)
        snapshot.runtime.append(record("record-source", "Recorded source", session.url.path, ["path": session.url.path, "format": "Codex rollout JSONL"]))
        snapshot.runtime.append(record("session-header", "Session identity", snapshot.agent ?? "Recorded Codex session", header))
        if let base = info["base_instructions"] { snapshot.context.append(record("base-instructions", "Recorded base instructions", render(base), base)) }
        var tools: [TelemetryToolEvent] = []
        var details: [[String: Any]] = []
        var indices: [String: Int] = [:]
        var responseIDs: [String: Int] = [:]
        var hiddenReasoningCount = 0
        var latestThreadUsage: TelemetryUsage?
        var latestTokenCount: TelemetryUsage?
        var responseUsage: [String: TelemetryUsage] = [:]
        var unknown = Set<String>()

        for (index, event) in events.enumerated() {
            let type = string(event["type"]) ?? "unknown"
            let data = event["payload"] as? [String: Any] ?? [:]
            let kind = string(data["type"]) ?? "unknown"
            let timestamp = date(event["timestamp"])
            let eventID = "codex-event-\(index)"
            switch type {
            case "session_meta": break
            case "turn_context":
                snapshot.model = string(data["model"]).map(clean) ?? snapshot.model
                snapshot.runtime.append(record(eventID, "Turn configuration", [string(data["model"]), string(data["effort"])].compactMap { $0 }.joined(separator: " · "), event))
                snapshot.context.append(record(eventID + "-context", "Recorded turn context", string(data["summary"]) ?? "Workspace, model, permissions and turn settings", event))
            case "response_item":
                if hidden(data) { hiddenReasoningCount += 1; continue }
                if kind == "message" || kind == "agent_message" {
                    let role = string(data["role"]) ?? (kind == "agent_message" ? "agent message" : "unknown")
                    let text = render(data["content"] ?? data["text"] ?? "")
                    snapshot.messages.append(TelemetryMessage(id: eventID, type: clean(role), text: text, timestamp: timestamp, rawJSON: pretty(event)))
                    if role == "user" {
                        if isHumanPrompt(text) { snapshot.taskPrompt = text }
                        snapshot.context.append(record(eventID, isHumanPrompt(text) ? "User prompt" : "Injected workspace instructions", text, event))
                    } else if role == "system" || role == "developer" {
                        snapshot.context.append(record(eventID, role.capitalized + " context", text, event))
                    } else if kind == "agent_message" {
                        snapshot.context.append(record(eventID + "-context", "Agent communication", text, event))
                    }
                } else if kind == "function_call" || kind == "custom_tool_call" {
                    guard let callID = string(data["call_id"]), !callID.isEmpty else { snapshot.warnings.append("A recorded tool call has no call ID."); continue }
                    if indices[callID] != nil { snapshot.warnings.append("A duplicate tool call ID was recorded: \(clean(callID))."); continue }
                    let name = [string(data["namespace"]), string(data["name"])].compactMap { $0 }.joined(separator: ".")
                    let input = decoded(data["arguments"] ?? data["input"] ?? "Not recorded")
                    indices[callID] = tools.count
                    if let responseID = string(data["id"]) { responseIDs[responseID] = tools.count }
                    tools.append(TelemetryToolEvent(id: callID, name: clean(name.isEmpty ? "Unknown tool" : name), status: "Result not recorded",
                        inputJSON: pretty(input), output: nil, error: nil, startedAt: timestamp, completedAt: nil, duration: nil, rawJSON: pretty(["call": event])))
                    details.append(["call": event])
                } else if kind == "function_call_output" || kind == "custom_tool_call_output" {
                    guard let callID = string(data["call_id"]), !callID.isEmpty else { snapshot.warnings.append("A recorded tool result has no call ID."); continue }
                    let toolIndex: Int
                    if let existing = indices[callID] { toolIndex = existing }
                    else {
                        toolIndex = tools.count; indices[callID] = toolIndex
                        tools.append(TelemetryToolEvent(id: callID, name: "Unknown tool", status: "Call not recorded", inputJSON: "Not recorded",
                            output: nil, error: nil, startedAt: nil, completedAt: nil, duration: nil, rawJSON: "{}"))
                        details.append([:]); snapshot.warnings.append("A recorded tool result has no corresponding call: \(clean(callID)).")
                    }
                    if details[toolIndex]["result"] != nil { snapshot.warnings.append("A duplicate tool result was recorded: \(clean(callID))."); continue }
                    let outputValue = data["output"] ?? "Not recorded"
                    let output = render(outputValue)
                    let failure = toolFailure(data) ?? toolFailure(decoded(outputValue))
                    tools[toolIndex].output = output
                    tools[toolIndex].error = failure
                    tools[toolIndex].status = failure == nil ? "Completed" : "Error"
                    tools[toolIndex].completedAt = timestamp
                    tools[toolIndex].duration = duration(tools[toolIndex].startedAt, timestamp)
                    details[toolIndex]["result"] = event
                    tools[toolIndex].rawJSON = pretty(details[toolIndex])
                    if tools[toolIndex].name.split(separator: ".").last == "spawn_agent",
                       let child = linkedChild(decoded(outputValue)) {
                        let input = decodedJSON(tools[toolIndex].inputJSON) as? [String: Any] ?? [:]
                        if let existing = snapshot.subagents.firstIndex(where: { $0.sessionID == child }) {
                            snapshot.subagents[existing].prompt = string(input["message"]).map(clean) ?? snapshot.subagents[existing].prompt
                            snapshot.subagents[existing].agent = string(input["agent_type"]).map(clean) ?? snapshot.subagents[existing].agent
                            if let title = string(input["task_name"]) { snapshot.subagents[existing].title = clean(title) }
                            snapshot.subagents[existing].rawJSON = pretty(["recordedLinks": [decodedJSON(snapshot.subagents[existing].rawJSON), details[toolIndex]]])
                        } else {
                            snapshot.subagents.append(TelemetrySubagent(id: child, title: clean(string(input["task_name"]) ?? "Recorded subagent"), sessionID: child,
                                agent: string(input["agent_type"]).map(clean), status: failure == nil ? "Spawn recorded" : "Spawn error",
                                prompt: string(input["message"]).map(clean), rawJSON: pretty(details[toolIndex])))
                        }
                    }
                } else {
                    unknown.insert("response_item/" + kind)
                    snapshot.runtime.append(record(eventID, "Recorded response item: \(kind)", "Inspect retained event", event))
                }
            case "token_usage_record":
                // thread_token_usage is cumulative for this thread. session_id
                // can point at the root for children; thread_id identifies it.
                if let thread = string(data["thread_id"]), thread != id { continue }
                if let usage = usage(data["thread_token_usage"]) { latestThreadUsage = usage }
                if let responseID = string(data["response_id"]), let usage = usage(data["usage"]) { responseUsage[responseID] = usage }
            case "event_msg":
                if kind == "token_count" {
                    let info = data["info"] as? [String: Any] ?? [:]
                    if let usage = usage(info["total_token_usage"]) { latestTokenCount = usage }
                    upsert(record("token-count", "Recorded token/context usage", "Latest cumulative token-count event", event), in: &snapshot.runtime)
                } else if kind == "item_completed" {
                    let item = data["item"] as? [String: Any] ?? [:]
                    let itemKind = string(item["type"]) ?? "unknown"
                    if itemKind.lowercased() == "reasoning" { continue }
                    if ["CommandExecution", "McpToolCall", "FileChange"].contains(itemKind) {
                        let itemID = string(item["id"]) ?? eventID
                        let start = date(data["started_at_ms"])
                        let end = date(data["completed_at_ms"]) ?? timestamp
                        let failure = toolFailure(item)
                        if let existing = indices[itemID] ?? responseIDs[itemID] {
                            details[existing]["completedItem"] = event
                            tools[existing].startedAt = start ?? tools[existing].startedAt
                            tools[existing].completedAt = end ?? tools[existing].completedAt
                            tools[existing].duration = duration(tools[existing].startedAt, tools[existing].completedAt)
                            if let failure { tools[existing].error = failure; tools[existing].status = "Error" }
                            tools[existing].rawJSON = pretty(details[existing])
                        } else {
                            let name = itemKind == "McpToolCall" ? [string(item["server"]), string(item["tool"])].compactMap { $0 }.joined(separator: ".") : itemKind
                            let input = item["arguments"] ?? item["command"] ?? item["changes"] ?? "Not recorded"
                            let output = item["aggregated_output"] ?? item["result"] ?? item["stdout"]
                            indices[itemID] = tools.count
                            tools.append(TelemetryToolEvent(id: itemID, name: clean(name), status: failure == nil ? clean(string(item["status"]) ?? "Completed") : "Error",
                                inputJSON: pretty(decoded(input)), output: output.map(render), error: failure, startedAt: start, completedAt: end,
                                duration: duration(start, end), rawJSON: pretty(event)))
                            details.append(["completedItem": event])
                        }
                    } else if itemKind == "SubAgentActivity", let child = string(item["agent_thread_id"]), isSessionID(child) {
                        let activity = string(item["kind"]) ?? "unknown"
                        let status = activity == "completed" ? "Completion recorded" : activity == "started" ? "Start recorded" : "Interaction recorded"
                        snapshot.runtime.append(record(eventID, "Subagent \(activity)", string(item["agent_path"]) ?? child, event))
                        if let existing = snapshot.subagents.firstIndex(where: { $0.sessionID == child }) {
                            snapshot.subagents[existing].status = status
                            snapshot.subagents[existing].rawJSON = pretty(["recordedLinks": [decodedJSON(snapshot.subagents[existing].rawJSON), event]])
                        } else if activity == "started" || sessions.contains(where: { $0.id == child && $0.parentSessionID == id }) {
                            snapshot.subagents.append(TelemetrySubagent(id: child, title: clean(string(item["agent_path"]) ?? "Recorded subagent"), sessionID: child,
                                agent: string(item["model"]).map(clean), status: status, prompt: nil, rawJSON: pretty(event)))
                        }
                    }
                    // User/assistant item-completion rows duplicate messages.
                } else if kind == "user_message" || kind == "agent_message" {
                    // Older Codex formats retain only these public messages.
                    let text = render(data["message"] ?? data["text"] ?? "")
                    if !snapshot.messages.contains(where: { $0.text == text && $0.timestamp == timestamp }) {
                        let role = kind == "user_message" ? "user" : "assistant"
                        snapshot.messages.append(TelemetryMessage(id: eventID, type: role, text: text, timestamp: timestamp, rawJSON: pretty(event)))
                        if role == "user", isHumanPrompt(text) { snapshot.taskPrompt = text }
                    }
                } else {
                    snapshot.runtime.append(record(eventID, "Codex \(kind.replacingOccurrences(of: "_", with: " "))", display(data), event))
                }
            case "compacted":
                snapshot.context.append(record(eventID, "Recorded context compaction", "Retained context and compaction metadata", event))
            case "inter_agent_communication_metadata":
                snapshot.context.append(record(eventID, "Agent communication metadata", "Recorded routing and trigger metadata", event))
            case "world_state":
                // Internal snapshots can contain nested reasoning. Public
                // messages, turn context and tool records cover visible work.
                break
            default:
                unknown.insert(type)
                snapshot.runtime.append(record(eventID, "Recorded event: \(type)", "Inspect retained event", event))
            }
        }
        for child in sessions where child.parentSessionID == id && normalized(child.project) == normalized(cwd) {
            if let existing = snapshot.subagents.firstIndex(where: { $0.sessionID == child.id }) {
                snapshot.subagents[existing].title = clean(child.title)
                snapshot.subagents[existing].agent = child.agent.map(clean) ?? snapshot.subagents[existing].agent
            } else {
                snapshot.subagents.append(TelemetrySubagent(id: child.id, title: clean(child.title), sessionID: child.id,
                    agent: child.agent.map(clean), status: "Recorded child session", prompt: nil,
                    rawJSON: pretty(["sessionID": child.id, "parentSessionID": id, "path": child.url.path, "relationshipSource": "session_meta.source.subagent.thread_spawn"])))
            }
        }
        snapshot.toolEvents = tools.sorted {
            ($0.startedAt ?? $0.completedAt ?? .distantPast) < ($1.startedAt ?? $1.completedAt ?? .distantPast)
        }
        snapshot.usage = latestThreadUsage ?? latestTokenCount ?? summed(Array(responseUsage.values))
        if latestThreadUsage != nil { snapshot.runtime.append(record("usage-source", "Token usage source", "Latest recorded thread_token_usage (cumulative)", ["source": "thread_token_usage"])) }
        else if latestTokenCount != nil { snapshot.runtime.append(record("usage-source", "Token usage source", "Latest recorded token_count total (cumulative)", [:])) }
        else if !responseUsage.isEmpty { snapshot.runtime.append(record("usage-source", "Token usage source", "Sum of unique recorded response usage", ["responseCount": responseUsage.count])) }
        if snapshot.title.isEmpty || snapshot.title.hasPrefix("Codex session ") {
            snapshot.title = snapshot.taskPrompt.map { String($0.prefix(100)) } ?? snapshot.title
        }
        snapshot.context.append(record("context-coverage", "Context coverage", "Recorded instructions, prompts, turn configuration and agent messages", [
            "coverage": "Only locally retained events are available; complete provider requests/tool schemas are not guaranteed. Hidden model reasoning and encrypted content are not displayed.",
            "hiddenReasoningItemCount": hiddenReasoningCount, "eventCount": events.count
        ]))
        if let notice = session.catalogNotice { snapshot.warnings.append(notice) }
        if partial { snapshot.warnings.append("The final live JSONL record is incomplete. Showing complete recorded events.") }
        if let malformed { snapshot.error = "The Codex rollout contains an invalid JSONL record at line \(malformed). Only the valid preceding events are displayed." }
        if snapshot.usage == nil { snapshot.warnings.append("No token usage was recorded for this session.") }
        if snapshot.model == nil { snapshot.warnings.append("No model was recorded for this session.") }
        if snapshot.provider == nil { snapshot.warnings.append("No provider was recorded for this session.") }
        snapshot.warnings.append("No cost was recorded. Token usage is shown without an estimated price.")
        if !unknown.isEmpty { snapshot.warnings.append("Unrecognized event schemas were retained as redacted records: \(unknown.sorted().joined(separator: ", ")).") }
        return snapshot
    }

    private static func unavailable(_ session: SessionDescriptor, _ error: String, _ collectedAt: Date = Date()) -> TelemetrySnapshot {
        var value = TelemetrySnapshot(sessionID: session.id, title: clean(session.title), project: clean(session.project), collectedAt: collectedAt)
        value.source = "Codex local rollout"; value.error = error
        return value
    }
    private static func safe(_ url: URL, beneath root: URL) -> Bool {
        let base = root.standardizedFileURL; let path = url.standardizedFileURL
        return (path.path == base.path || path.path.hasPrefix(base.path + "/")) &&
            base.resolvingSymlinksInPath().path == base.path && path.resolvingSymlinksInPath().path == path.path
    }
    private static func regular(_ url: URL) -> Bool {
        guard let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]) else { return false }
        return values.isRegularFile == true && values.isSymbolicLink != true
    }
    private static func modified(_ url: URL) -> Date { (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast }
    private static func prefix(_ url: URL, count: Int) throws -> Data {
        let handle = try FileHandle(forReadingFrom: url); defer { try? handle.close() }
        return try handle.read(upToCount: count) ?? Data()
    }
    private static func normalized(_ path: String) -> String { URL(fileURLWithPath: path).standardizedFileURL.path }
    private static func object(_ data: Data) -> [String: Any]? { (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] }
    private static func isSessionID(_ value: String) -> Bool { UUID(uuidString: value) != nil }
    private static func string(_ value: Any?) -> String? {
        if let text = value as? String { return text }
        if let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() { return number.stringValue }
        return nil
    }
    private static func spawnMetadata(_ info: [String: Any]) -> [String: Any] {
        let source = info["source"] as? [String: Any] ?? [:]
        let subagent = source["subagent"] as? [String: Any] ?? [:]
        return subagent["thread_spawn"] as? [String: Any] ?? [:]
    }
    private static func internalSession(_ info: [String: Any]) -> Bool {
        let source = info["source"] as? [String: Any] ?? [:]
        let subagent = source["subagent"] as? [String: Any] ?? [:]
        let discriminants = [string(info["source"]), string(info["thread_source"]), string(info["originator"]), string(info["agent_role"])]
            .compactMap { $0?.lowercased() }
        return subagent.keys.contains { ["guardian", "auto_review", "auto-review", "approval_review"].contains($0.lowercased()) } ||
            discriminants.contains { ["guardian", "auto_review", "auto-review", "approval_review"].contains($0) }
    }
    private static func isHumanPrompt(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && !["# AGENTS.md instructions", "<environment_context>", "<permissions instructions>", "<INSTRUCTIONS>"]
            .contains(where: trimmed.hasPrefix)
    }
    private static func date(_ value: Any?) -> Date? {
        if let number = value as? NSNumber { return Date(timeIntervalSince1970: number.doubleValue > 1e11 ? number.doubleValue / 1_000 : number.doubleValue) }
        guard let text = value as? String else { return nil }
        let formatter = ISO8601DateFormatter(); formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: text) ?? ISO8601DateFormatter().date(from: text)
    }
    private static func duration(_ start: Date?, _ end: Date?) -> TimeInterval? { start.flatMap { start in end.flatMap { $0 >= start ? $0.timeIntervalSince(start) : nil } } }
    private static func decodedJSON(_ text: String) -> Any { (try? JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed])) ?? text }
    private static func decoded(_ value: Any) -> Any { (value as? String).map(decodedJSON) ?? value }
    private static func clean(_ text: String) -> String { AgentTelemetry.redactedText(text) }
    private static func hidden(_ object: [String: Any]) -> Bool {
        ["type", "channel", "phase"].contains { key in ["reasoning", "analysis"].contains(string(object[key])?.lowercased() ?? "") }
    }
    private static func publicValue(_ value: Any) -> Any {
        if let object = value as? [String: Any] {
            if hidden(object) { return ["type": string(object["type"]) ?? "reasoning", "content": "Hidden model reasoning is not displayed."] }
            var result: [String: Any] = [:]
            for (key, value) in object where !["encrypted_content", "raw_content", "reasoning_content", "reasoning_text"].contains(key.lowercased()) {
                result[key] = publicValue(value)
            }
            return result
        }
        if let array = value as? [Any] { return array.map(publicValue) }
        return value
    }
    private static func pretty(_ value: Any) -> String { AgentTelemetry.redactedJSON(publicValue(value)) }
    private static func render(_ value: Any) -> String {
        if let text = value as? String { return clean(text) }
        if let array = value as? [Any] { return array.map(render).filter { !$0.isEmpty }.joined(separator: "\n\n") }
        if let object = value as? [String: Any] {
            if hidden(object) { return "" }
            if let text = string(object["text"]) { return clean(text) }
        }
        return pretty(value)
    }
    private static func record(_ id: String, _ title: String, _ value: String, _ raw: Any) -> TelemetryRecord {
        TelemetryRecord(id: id, title: clean(title), value: String(clean(value).prefix(240)), detailJSON: pretty(raw))
    }
    private static func upsert(_ value: TelemetryRecord, in records: inout [TelemetryRecord]) {
        if let index = records.firstIndex(where: { $0.id == value.id }) { records[index] = value } else { records.append(value) }
    }
    private static func display(_ data: [String: Any]) -> String {
        let keys = ["status", "model", "effort", "duration_ms", "error"]
        let values = keys.compactMap { key in string(data[key]).map { "\(key): \($0)" } }
        return values.isEmpty ? "Recorded event · inspect details" : clean(values.joined(separator: " · "))
    }
    private static func usage(_ value: Any?) -> TelemetryUsage? {
        guard let object = value as? [String: Any] else { return nil }
        func number(_ keys: [String]) -> Int64? {
            for key in keys { if let text = string(object[key]), let value = Int64(text), value >= 0 { return value } }
            return nil
        }
        let result = TelemetryUsage(input: number(["input_tokens"]), output: number(["output_tokens"]),
            reasoning: number(["reasoning_output_tokens", "reasoning_tokens"]), cacheRead: number(["cached_input_tokens"]),
            cacheWrite: number(["cache_write_input_tokens"]), total: number(["total_tokens"]))
        return [result.input, result.output, result.reasoning, result.cacheRead, result.cacheWrite, result.total].contains { $0 != nil } ? result : nil
    }
    private static func summed(_ values: [TelemetryUsage]) -> TelemetryUsage? {
        guard !values.isEmpty else { return nil }
        func total(_ key: KeyPath<TelemetryUsage, Int64?>) -> Int64? {
            let known = values.compactMap { $0[keyPath: key] }
            guard !known.isEmpty else { return nil }
            var sum: Int64 = 0
            for value in known { let next = sum.addingReportingOverflow(value); guard !next.overflow else { return nil }; sum = next.partialValue }
            return sum
        }
        return TelemetryUsage(input: total(\.input), output: total(\.output), reasoning: total(\.reasoning), cacheRead: total(\.cacheRead), cacheWrite: total(\.cacheWrite), total: total(\.total))
    }
    private static func toolFailure(_ value: Any, depth: Int = 0) -> String? {
        guard depth < 8 else { return nil }
        if let object = value as? [String: Any] {
            if object["isError"] as? Bool == true || object["is_error"] as? Bool == true { return render(object["error"] ?? object["content"] ?? object["output"] ?? "Recorded tool error") }
            if let error = object["error"], !(error is NSNull), render(error) != "" { return render(error) }
            if let code = string(object["exit_code"]).flatMap(Int.init), code != 0 { return "Recorded process exit code \(code)" }
            if ["failed", "error"].contains(string(object["status"])?.lowercased() ?? "") { return "Recorded tool status: \(string(object["status"])!)" }
            for key in ["output", "content", "result", "text"] { if let nested = object[key], let failure = toolFailure(decoded(nested), depth: depth + 1) { return failure } }
        } else if let array = value as? [Any] {
            for item in array { if let failure = toolFailure(item, depth: depth + 1) { return failure } }
        } else if let text = value as? String {
            let parsed = decodedJSON(text)
            if !(parsed is String) { return toolFailure(parsed, depth: depth + 1) }
            if let range = text.range(of: #"(?m)^Process exited with code [1-9][0-9]*\s*$"#, options: .regularExpression) { return clean(String(text[range])) }
        }
        return nil
    }
    private static func linkedChild(_ value: Any, depth: Int = 0) -> String? {
        guard depth < 8 else { return nil }
        if let object = value as? [String: Any] {
            for key in ["agent_id", "agentId", "thread_id", "session_id"] {
                if let id = string(object[key]), isSessionID(id) { return id }
            }
            for key in ["output", "content", "result", "text"] { if let nested = object[key], let id = linkedChild(decoded(nested), depth: depth + 1) { return id } }
        } else if let array = value as? [Any] {
            for item in array { if let id = linkedChild(item, depth: depth + 1) { return id } }
        } else if let text = value as? String {
            let decoded = decodedJSON(text)
            if !(decoded is String) { return linkedChild(decoded, depth: depth + 1) }
        }
        return nil
    }
}
