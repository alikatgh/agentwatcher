// Synthetic recorded telemetry only: no agent, provider or live session access.
// verify_changes.py appends this file to the actual application sources.
func runTelemetryChecks(scratchURL: URL) {
    func json(_ value: Any) -> String {
        String(data: try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), encoding: .utf8)!
    }
    func event(_ type: String, _ seq: Int, _ time: Int64, _ data: [String: Any]) -> [String: Any] {
        ["type": type, "seq": seq, "time": time, "data": data]
    }
    func encoded(_ value: TelemetrySnapshot) -> String {
        String(data: try! JSONEncoder().encode(value), encoding: .utf8)!
    }
    let base: Int64 = 1_791_900_000_000
    let session = DeepSeekTelemetry.SessionDescriptor(id: "session-fixture", title: "Untitled session", project: "/fixture",
        url: scratchURL.appendingPathComponent("session-fixture/session.v4.jsonl"), updated: Date(timeIntervalSince1970: Double(base) / 1_000))
    let header: [String: Any] = ["type": "session", "version": 4, "id": "session-fixture", "createdAt": base,
        "cwd": "/fixture", "isSeeded": false, "delegationDepth": 0, "agentPreset": "standard"]
    let schema: [String: Any] = ["name": "bash", "description": "Run a command",
        "parameters": ["type": "object", "properties": ["command": ["type": "string"]], "required": ["command"]]]
    let rows: [[String: Any]] = [header,
        event("permission/preset", 0, base + 1, ["preset": "workspace-write"]),
        event("sandbox/mode", 1, base + 2, ["mode": "workspace-write"]),
        event("approval/policy", 2, base + 3, ["policy": "ask"]),
        event("model/selection", 3, base + 4, ["provider": "deepseek-official", "model": "deepseek-flash", "reasoningEffort": "max"]),
        event("turn/start", 4, base + 5, ["turn": 1]),
        event("step/start", 5, base + 6, ["turn": 1, "step": 2]),
        event("request/header", 6, base + 7, ["header": ["config": ["model": "deepseek-flash"], "tools": [schema]]]),
        event("request/context", 7, base + 8, ["provider": "deepseek-official", "model": "deepseek-flash", "contextWindow": 1_000_000, "systemPromptUpdate": "unchanged"]),
        event("system/message", 8, base + 9, ["turn": 1, "step": 2,
            "message": ["id": "system-1", "role": "system", "content": [["type": "text", "text": "Recorded system prompt"]]]]),
        event("user/message", 9, base + 10, ["id": "user-1", "role": "user", "content": [["type": "text", "text": "Inspect the fixture"]]]),
        event("assistant/message", 10, base + 11, ["turn": 1, "step": 2,
            "message": ["id": "assistant-1", "role": "assistant", "content": [["type": "reasoning", "text": "Recorded reasoning"], ["type": "text", "text": "Inspecting"]]],
            "usage": ["inputTokens": 100, "outputTokens": 20, "cacheReadTokens": 50, "cacheWriteTokens": 10, "totalTokens": 120]]),
        event("tool/call", 11, base + 1_000, ["turn": 1, "step": 2, "callId": "call-bash", "name": "bash", "arguments": json(["command": "pwd"])]),
        event("tool/result", 12, base + 3_000, ["turn": 1, "step": 2,
            "message": ["role": "tool", "source": ["kind": "tool", "callId": "call-bash"], "toolCallId": "call-bash", "content": [["type": "text", "text": "/fixture"]], "isError": false]]),
        event("tool/call", 13, base + 4_000, ["turn": 1, "step": 2, "callId": "call-child", "name": "subagent",
            "arguments": json(["description": "Review the fixture", "prompt": "Review only the supplied fixture", "model": "deepseek-flash"])]),
        event("subagent/catalog", 14, base + 4_100, ["version": 0, "childId": "session-child", "childCreatedAt": base + 4_050, "mode": "continuable", "label": "Review the fixture"]),
        event("tool/result", 15, base + 4_200, ["message": ["toolCallId": "call-child", "content": [["type": "text", "text": "started subagent session-child"]], "isError": false]]),
        event("tool/call", 16, base + 5_000, ["turn": 1, "step": 2, "callId": "call-error", "name": "read", "arguments": json(["path": "missing.txt"])]),
        event("tool/result", 17, base + 5_250, ["message": ["toolCallId": "call-error", "content": [["type": "text", "text": "File was not found"]], "isError": true]]),
        event("assistant/message", 18, base + 6_000, ["message": ["id": "assistant-2", "role": "assistant", "content": [["type": "text", "text": "Next step"]]],
            "usage": ["inputTokens": 25, "outputTokens": 5, "totalTokens": 30]]),
        event("tool/call", 19, base + 7_000, ["turn": 1, "step": 2, "callId": "call-pending", "name": "read", "arguments": json(["path": "pending.txt"])])
    ]
    let jsonl = rows.map(json).joined(separator: "\n") + "\n"
    let parsed = try! DeepSeekTelemetry.parseSessionEvents(jsonl, session: session)
    check(parsed.sessionID == session.id && parsed.project == "/fixture", "DSH v4 header identity/project lost")
    check(parsed.model == "deepseek-flash" && parsed.provider == "deepseek-official", "DSH recorded route lost")
    check(parsed.toolEvents.count == 4, "DSH call/result rows were counted as separate tools")
    let tool = parsed.toolEvents.first { $0.id == "call-bash" }!
    check(tool.output == "/fixture" && tool.status == "Completed", "DSH result did not pair with its exact call ID")
    check(tool.duration == 2 && tool.startedAt != nil && tool.completedAt != nil, "DSH recorded tool timestamps/duration lost")
    let details = try! JSONSerialization.jsonObject(with: Data(tool.rawJSON.utf8)) as! [String: Any]
    check((details["turn"] as? Int) == 1 && (details["step"] as? Int) == 2 && details["agent"] as? String == "standard", "DSH tool hierarchy lost")
    check((details["toolSchema"] as? [String: Any])?["name"] as? String == "bash", "DSH provider tool schema lost")
    check(details["call"] != nil && details["result"] != nil, "DSH inspector lost original call/result records")
    let pending = parsed.toolEvents.first { $0.id == "call-pending" }!
    check(pending.output == nil && pending.duration == nil && pending.completedAt == nil, "DSH unfinished tool acquired a fabricated result/timing")
    let failed = parsed.toolEvents.first { $0.id == "call-error" }!
    check(failed.status == "Error" && failed.error == "File was not found" && failed.duration == 0.25, "DSH recorded tool error lost")
    check(parsed.usage?.input == 125 && parsed.usage?.output == 25 && parsed.usage?.total == 150, "DSH assistant-message usage was not summed")
    check(parsed.usage?.cacheRead == 50 && parsed.usage?.cacheWrite == 10 && parsed.usage?.reasoning == nil, "DSH cache usage changed or absent reasoning was invented")
    check(parsed.recordedCost == nil && parsed.costCurrency == nil, "DSH absent cost/currency was invented")
    check(parsed.taskPrompt == "Inspect the fixture", "DSH user prompt lost")
    check(parsed.context.contains { $0.title == "System prompt" && $0.value == "Recorded system prompt" }, "DSH system context lost")
    check(parsed.context.contains { $0.title == "Provider request header" } && parsed.context.contains { $0.title == "Provider request context" }, "DSH request context lost")
    check(parsed.runtime.contains { $0.title == "sandbox/mode" } && parsed.runtime.contains { $0.title == "approval/policy" }, "DSH recorded permissions lost")
    check(parsed.subagents.count == 1 && parsed.subagents[0].sessionID == "session-child", "DSH exact catalog/spawn child link was lost or duplicated")
    check(parsed.subagents[0].prompt == "Review only the supplied fixture", "DSH delegated task prompt lost")
    check(parsed.toolEvents.first { $0.id == "call-child" }!.rawJSON.contains("subagentSessionID"), "DSH inspector child-session link lost")
    let partial = try! DeepSeekTelemetry.parseSessionEvents(jsonl + "{\"type\":", session: session)
    check(partial.toolEvents == parsed.toolEvents && partial.warnings.contains { $0.contains("incomplete") }, "DSH live partial final line was not tolerated explicitly")
    do {
        _ = try DeepSeekTelemetry.parseSessionEvents(jsonl + "invalid-interior\n{}\n", session: session)
        check(false, "DSH interior corruption silently accepted")
    } catch {}
    var wrongVersion = header; wrongVersion["version"] = 99
    do {
        _ = try DeepSeekTelemetry.parseSessionEvents(json(wrongVersion) + "\n", session: session)
        check(false, "DSH unsupported header version accepted")
    } catch {}
    let empty = try! DeepSeekTelemetry.parseSessionEvents(json(header) + "\n", session: session)
    check(empty.usage == nil && empty.recordedCost == nil && empty.model == nil && empty.toolEvents.isEmpty, "DSH unknown telemetry was fabricated")
    check(empty.warnings.contains { $0.contains("No assistant token usage") }, "DSH missing token usage was not explained")
    let promptRows = [header,
        event("user/message", 0, base, ["id": "human-first", "role": "user", "source": ["kind": "user", "rpcId": "synthetic-first"], "content": [["type": "text", "text": "First human task"]]]),
        event("user/message", 1, base + 1, ["id": "runtime-first", "role": "user", "source": ["kind": "runtime-context", "form": "snapshot"], "content": [["type": "text", "text": "Current runtime context. This snapshot supersedes earlier runtime-context snapshots."]]]),
        event("user/message", 2, base + 2, ["id": "human-latest", "role": "user", "source": ["kind": "user", "rpcId": "synthetic-latest"], "content": [["type": "text", "text": "Latest human task"]]]),
        event("user/message", 3, base + 3, ["id": "runtime-latest", "role": "user", "source": ["kind": "runtime-context", "form": "snapshot"], "content": [["type": "text", "text": "Latest runtime injection"]]])
    ]
    let humanPrompt = try! DeepSeekTelemetry.parseSessionEvents(promptRows.map(json).joined(separator: "\n") + "\n", session: session)
    check(humanPrompt.taskPrompt == "Latest human task", "DSH runtime-context injection replaced the latest actual human task")
    check(humanPrompt.messages.count == 4 && humanPrompt.context.contains { $0.title == "runtime-context injection" && $0.value == "Latest runtime injection" }, "DSH prompt selection discarded injected context")
    let fallbackRows = [header,
        event("user/message", 0, base, ["id": "unclassified-first", "role": "user", "content": [["type": "text", "text": "First unclassified task"]]]),
        event("user/message", 1, base + 1, ["id": "unclassified-later", "role": "user", "content": [["type": "text", "text": "Later unclassified text"]]])
    ]
    let fallbackPrompt = try! DeepSeekTelemetry.parseSessionEvents(fallbackRows.map(json).joined(separator: "\n") + "\n", session: session)
    check(fallbackPrompt.taskPrompt == "First unclassified task", "DSH source-free fallback did not preserve the first recorded task")
    print("PASS: DSH v4 header, exact tool pairing/schema/hierarchy/timing, errors, partial live records, catalog links and known/unknown usage")
    print("PASS: DSH latest human source wins; runtime-context injections remain inspectable and unclassified prompts fall back to the first")

    let openCode: [String: Any] = [
        "info": ["id": "ses_fixture", "title": "Parts-only prompt", "directory": "/fixture"],
        "messages": [["info": ["id": "user-parts", "role": "user", "time": ["created": base]],
            "parts": [["type": "text", "text": "Inspect the first part"], ["type": "text", "text": "Then inspect the second part"]]]]
    ]
    let openCodeParsed = try! AgentTelemetry.parseOpenCodeExport(json(openCode), project: "/fixture")
    check(openCodeParsed.taskPrompt == "Inspect the first part\n\nThen inspect the second part", "OpenCode parts-only user prompt missing from overview")
    check(openCodeParsed.context.contains { $0.title.hasPrefix("User prompt") && $0.value.contains("Inspect the first part") }, "OpenCode parts-only prompt missing from context")
    check(openCodeParsed.usage == nil && openCodeParsed.recordedCost == nil, "OpenCode absent usage/cost was invented")
    print("PASS: OpenCode parts-only prompt retained in overview and context; unknown usage/cost remain absent")

    // Test the whole serialized snapshot: rawJSON/detailJSON/inputJSON are JSON
    // strings embedded in Codable output, so checking just inputJSON misses leaks.
    let markers = ["FAKE_OPENAI_QUOTED_MARKER_61", "FAKE_GITHUB_TOKEN_MARKER_62", "FAKE_AWS_SESSION_MARKER_63", "FAKE_AWS_ACCESS_MARKER_64", "FAKE_NESTED_ENV_MARKER_65"]
    let command = "export OPENAI_API_KEY=\"prefix \(markers[0])\" GITHUB_TOKEN=\"\(markers[1])\" AWS_SESSION_TOKEN='\(markers[2])' AWS_ACCESS_KEY_ID=\"\(markers[3])\"; pwd"
    let nestedJSON = json(["command": command, "environment": ["OPENAI_API_KEY": markers[4]], "GITHUB_TOKEN": markers[1], "AWS_SESSION_TOKEN": markers[2], "AWS_ACCESS_KEY_ID": markers[3]])
    let privateArguments: [String: Any] = ["command": command, "nested": nestedJSON, "env": ["OPENAI_API_KEY": markers[4]]]
    let privateRows: [[String: Any]] = [header,
        event("user/message", 0, base, ["id": "private-user", "role": "user", "content": [["type": "text", "text": command]]]),
        event("tool/call", 1, base + 1, ["turn": 1, "step": 1, "callId": "private-call", "name": "bash", "arguments": json(privateArguments)]),
        event("tool/result", 2, base + 2, ["message": ["toolCallId": "private-call", "content": [["type": "text", "text": nestedJSON]], "isError": false]])
    ]
    let privateDSH = try! DeepSeekTelemetry.parseSessionEvents(privateRows.map(json).joined(separator: "\n") + "\n", session: session)
    let privateOpenCode: [String: Any] = [
        "info": ["id": "ses_private_fixture", "title": "Private fixture", "directory": "/fixture"],
        "messages": [["info": ["id": "private-user", "role": "user"], "parts": [["type": "text", "text": command]]],
            ["info": ["id": "private-assistant", "role": "assistant"], "parts": [["id": "private-tool", "type": "tool", "name": "bash", "state": ["status": "completed", "input": privateArguments, "output": nestedJSON]]]]]
    ]
    let privateOC = try! AgentTelemetry.parseOpenCodeExport(json(privateOpenCode), project: "/fixture")
    for (source, snapshot) in [("DSH", privateDSH), ("OpenCode", privateOC)] {
        let wholeSnapshot = encoded(snapshot)
        for marker in markers { check(!wholeSnapshot.contains(marker), "\(source) whole encoded snapshot leaked a credential marker: \(marker)") }
        check(wholeSnapshot.contains("redacted"), "\(source) credential fixture did not exercise redaction")
        check(snapshot.toolEvents.count == 1 && !snapshot.messages.isEmpty, "\(source) privacy check passed by dropping the recorded session")
    }
    print("PASS: complete Codable snapshots redact quoted OPENAI_API_KEY, GITHUB_TOKEN, AWS_SESSION_TOKEN, AWS_ACCESS_KEY_ID and nested JSON-string environment values")
}
