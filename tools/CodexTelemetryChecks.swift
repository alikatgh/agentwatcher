// Synthetic Codex recordings only. This function starts no agents or providers.
func runCodexTelemetryChecks(scratchURL: URL) {
    func json(_ value: Any) -> String {
        String(data: try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), encoding: .utf8)!
    }
    func event(_ type: String, _ timestamp: String, _ payload: [String: Any]) -> [String: Any] {
        ["type": type, "timestamp": timestamp, "payload": payload]
    }
    func encoded(_ snapshot: TelemetrySnapshot) -> String {
        String(data: try! JSONEncoder().encode(snapshot), encoding: .utf8)!
    }
    let parentID = "10000000-0000-0000-0000-000000000001"
    let childID = "10000000-0000-0000-0000-000000000002"
    let otherID = "10000000-0000-0000-0000-000000000003"
    let base = "2026-10-14T01:00:00.000Z"
    let session = CodexTelemetry.SessionDescriptor(id: parentID, title: "Codex session fixture", project: "/fixture",
        url: scratchURL.appendingPathComponent("rollout-fixture-\(parentID).jsonl"), updated: Date())
    let child = CodexTelemetry.SessionDescriptor(id: childID, title: "Review fixture", project: "/fixture",
        url: scratchURL.appendingPathComponent("rollout-fixture-\(childID).jsonl"), updated: Date(), parentSessionID: parentID, agent: "Reviewer")
    let header = event("session_meta", base, ["id": parentID, "cwd": "/fixture", "source": "vscode", "model_provider": "openai",
        "base_instructions": ["text": "Recorded base instructions"]])
    let usageA: [String: Any] = ["input_tokens": 100, "output_tokens": 10, "reasoning_output_tokens": 3,
        "cached_input_tokens": 50, "cache_write_input_tokens": 0, "total_tokens": 110]
    let usageB: [String: Any] = ["input_tokens": 125, "output_tokens": 15, "reasoning_output_tokens": 4,
        "cached_input_tokens": 60, "cache_write_input_tokens": 0, "total_tokens": 140]
    let rows = [header,
        event("response_item", base, ["type": "message", "role": "user", "content": [["type": "input_text", "text": "# AGENTS.md instructions\nInjected instructions"]]]),
        event("response_item", base, ["type": "message", "role": "developer", "content": [["type": "input_text", "text": "Recorded developer context"]]]),
        event("response_item", base, ["type": "message", "role": "user", "content": [["type": "input_text", "text": "Inspect the fixture"]]]),
        event("turn_context", base, ["model": "gpt-6-astra", "effort": "high", "sandbox_policy": ["type": "workspace-write"], "approval_policy": "on-request"]),
        event("response_item", "2026-10-14T01:00:01.000Z", ["type": "function_call", "id": "item-a", "call_id": "call-a", "namespace": "functions", "name": "exec", "arguments": json(["command": "pwd"]) ]),
        event("response_item", "2026-10-14T01:00:01.500Z", ["type": "custom_tool_call", "id": "item-b", "call_id": "call-b", "name": "apply_patch", "input": "fixture patch"]),
        event("response_item", "2026-10-14T01:00:02.000Z", ["type": "custom_tool_call_output", "call_id": "call-b", "output": [["type": "text", "text": "Patch applied"]]]),
        event("response_item", "2026-10-14T01:00:03.000Z", ["type": "function_call_output", "call_id": "call-a", "output": "/fixture"]),
        event("response_item", "2026-10-14T01:00:04.000Z", ["type": "function_call", "call_id": "call-error", "name": "read", "arguments": "{}"]),
        event("response_item", "2026-10-14T01:00:04.500Z", ["type": "function_call_output", "call_id": "call-error", "output": json(["isError": true, "content": "Missing file"])]),
        event("response_item", "2026-10-14T01:00:05.000Z", ["type": "function_call", "call_id": "call-pending", "name": "read", "arguments": "{}"]),
        event("response_item", "2026-10-14T01:00:06.000Z", ["type": "function_call", "call_id": "call-child", "namespace": "collaboration", "name": "spawn_agent", "arguments": json(["task_name": "fixture-review", "message": "Review only the fixture", "agent_type": "worker"])]),
        event("event_msg", "2026-10-14T01:00:06.250Z", ["type": "item_completed", "item": ["type": "SubAgentActivity", "agent_thread_id": childID, "kind": "started", "agent_path": "/root/reviewer"]]),
        event("response_item", "2026-10-14T01:00:06.500Z", ["type": "function_call_output", "call_id": "call-child", "output": [["type": "text", "text": json(["agent_id": childID])]]]),
        event("response_item", "2026-10-14T01:00:07.000Z", ["type": "message", "role": "assistant", "content": [["type": "output_text", "text": "Inspecting the recorded fixture"]]]),
        event("response_item", base, ["type": "reasoning", "encrypted_content": "NEVER_SHOW_ENCRYPTED", "summary": [["text": "NEVER_SHOW_REASONING"]]]),
        event("response_item", base, ["type": "message", "role": "assistant", "channel": "analysis", "content": [["text": "NEVER_SHOW_ANALYSIS"]]]),
        event("event_msg", base, ["type": "item_completed", "item": ["type": "Reasoning", "raw_content": "NEVER_SHOW_RAW", "summary_text": "NEVER_SHOW_SUMMARY"]]),
        event("token_usage_record", base, ["thread_id": parentID, "response_id": "response-a", "usage": usageA, "thread_token_usage": usageA]),
        event("event_msg", base, ["type": "token_count", "info": ["total_token_usage": usageA]]),
        event("token_usage_record", base, ["thread_id": parentID, "response_id": "response-b", "usage": usageA, "thread_token_usage": usageB]),
        event("event_msg", base, ["type": "token_count", "info": ["total_token_usage": usageB]]),
        event("event_msg", "2026-10-14T01:00:08.000Z", ["type": "task_complete", "duration_ms": 8000, "error": NSNull()])
    ]
    let jsonl = rows.map(json).joined(separator: "\n") + "\n"
    let parsed = CodexTelemetry.parseSessionEvents(jsonl, session: session, sessions: [session, child])
    check(parsed.error == nil && parsed.sessionID == parentID && parsed.project == "/fixture", "Codex header identity/project lost")
    check(parsed.model == "gpt-6-astra" && parsed.provider == "openai", "Codex recorded model/provider lost")
    check(parsed.taskPrompt == "Inspect the fixture" && parsed.title == "Inspect the fixture", "Codex injected instructions replaced the human task")
    check(parsed.toolEvents.count == 5, "Codex call and result were counted as separate tools")
    let first = parsed.toolEvents.first { $0.id == "call-a" }!
    let second = parsed.toolEvents.first { $0.id == "call-b" }!
    check(first.output == "/fixture" && second.output == "Patch applied", "Codex interleaved results failed exact call-ID pairing")
    check(first.name == "functions.exec" && first.duration == 2 && second.duration == 0.5, "Codex tool name or recorded timing lost")
    check(first.rawJSON.contains("\"call\"") && first.rawJSON.contains("\"result\""), "Codex tool details lost original call/result events")
    let failed = parsed.toolEvents.first { $0.id == "call-error" }!
    check(failed.status == "Error" && failed.error == "Missing file", "Codex recorded tool error was not shown")
    let pending = parsed.toolEvents.first { $0.id == "call-pending" }!
    check(pending.output == nil && pending.completedAt == nil && pending.duration == nil, "Codex pending tool got fabricated completion")
    check(parsed.usage?.input == 125 && parsed.usage?.output == 15 && parsed.usage?.total == 140, "Codex cumulative usage was double counted")
    check(parsed.usage?.reasoning == 4 && parsed.usage?.cacheRead == 60 && parsed.usage?.cacheWrite == 0, "Codex recorded usage categories lost")
    check(parsed.recordedCost == nil && parsed.costCurrency == nil, "Codex unrecorded cost was invented")
    check(parsed.subagents.count == 1 && parsed.subagents.first?.sessionID == childID && parsed.subagents.first?.title == "Review fixture", "Codex catalog/spawn links were lost or duplicated")
    check(parsed.subagents.first?.prompt == "Review only the fixture", "Codex delegated prompt lost")
    check(parsed.context.contains { $0.title == "Developer context" } && parsed.context.contains { $0.title == "Recorded base instructions" }, "Codex recorded instructions lost")
    check(parsed.runtime.contains { $0.detailJSON.contains("approval_policy") && $0.detailJSON.contains("sandbox_policy") }, "Codex recorded permissions lost")
    check(!encoded(parsed).contains("NEVER_SHOW"), "Codex hidden reasoning or encrypted content was exposed")
    let catalogOnly = CodexTelemetry.parseSessionEvents(json(header) + "\n", session: session, sessions: [session, child])
    check(catalogOnly.subagents.count == 1 && catalogOnly.subagents.first?.sessionID == childID, "Codex child metadata requires a parent spawn call")
    check(catalogOnly.model == nil && catalogOnly.usage == nil && catalogOnly.recordedCost == nil, "Codex missing metrics were fabricated")
    check(catalogOnly.warnings.contains { $0.contains("No token usage") } && catalogOnly.warnings.contains { $0.contains("No model") }, "Codex missing metrics lack explanations")
    let partial = CodexTelemetry.parseSessionEvents(jsonl + "{\"type\":", session: session)
    check(partial.error == nil && partial.toolEvents.count == 5 && partial.warnings.contains { $0.contains("incomplete") }, "Codex partial live JSONL was not handled explicitly")
    let malformed = CodexTelemetry.parseSessionEvents(jsonl + "invalid\n{}\n", session: session)
    check(malformed.error?.contains("invalid JSONL") == true && malformed.toolEvents.count == 5, "Codex interior corruption was silently accepted")
    var wrong = session; wrong.project = "/other"
    check(CodexTelemetry.parseSessionEvents(jsonl, session: wrong).error?.contains("different workspace") == true, "Codex cross-project rollout accepted")
    let orphan = CodexTelemetry.parseSessionEvents([header, event("response_item", base, ["type": "function_call_output", "call_id": "orphan", "output": "Recorded result"])].map(json).joined(separator: "\n"), session: session)
    check(orphan.toolEvents.first?.startedAt == nil && orphan.toolEvents.first?.output == "Recorded result" && orphan.warnings.contains { $0.contains("no corresponding call") }, "Codex orphan result acquired an invented call")
    let secrets = [header,
        event("response_item", base, ["type": "function_call", "call_id": "secret", "name": "read", "arguments": json(["api_key": "codex-fixture-secret-value", "url": "https://example.invalid/?token=codex-query-secret"]) ]),
        event("response_item", base, ["type": "function_call_output", "call_id": "secret", "output": "Authorization: Bearer codex-fixture-bearer"])
    ]
    let secretText = encoded(CodexTelemetry.parseSessionEvents(secrets.map(json).joined(separator: "\n"), session: session))
    check(!secretText.contains("codex-fixture-secret-value") && !secretText.contains("codex-query-secret") && !secretText.contains("codex-fixture-bearer"), "Codex credential redaction failed")
    let innerRows = [header, event("event_msg", base, ["type": "item_completed", "started_at_ms": 1_000_000_000_000, "completed_at_ms": 1_000_000_001_250,
        "item": ["type": "CommandExecution", "id": "inner-command", "command": "pwd", "aggregated_output": "/fixture", "status": "completed", "exit_code": 2]])]
    let inner = CodexTelemetry.parseSessionEvents(innerRows.map(json).joined(separator: "\n"), session: session)
    check(inner.toolEvents.count == 1 && inner.toolEvents.first?.duration == 1.25 && inner.toolEvents.first?.status == "Error", "Codex inner command timing/exit status lost")

    // Discovery/reader fixtures stay within the caller's session-owned scratch.
    let home = scratchURL.appendingPathComponent("codex-home", isDirectory: true)
    let formatter = DateFormatter(); formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = TimeZone(secondsFromGMT: 0); formatter.dateFormat = "yyyy/MM/dd"
    let directory = home.appendingPathComponent("sessions/" + formatter.string(from: Date()), isDirectory: true)
    try! FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let parentFile = directory.appendingPathComponent("rollout-fixture-\(parentID).jsonl")
    try! Data(jsonl.utf8).write(to: parentFile)
    let childHeader = event("session_meta", base, ["id": childID, "cwd": "/fixture", "source": ["subagent": ["thread_spawn": ["parent_thread_id": parentID,
        "agent_path": "/root/reviewer", "agent_nickname": "Reviewer", "agent_role": "worker"]]]])
    let childFile = directory.appendingPathComponent("rollout-fixture-\(childID).jsonl")
    try! Data((json(childHeader) + "\n").utf8).write(to: childFile)
    try! Data((json(event("session_meta", base, ["id": otherID, "cwd": "/other", "source": "vscode"])) + "\n").utf8)
        .write(to: directory.appendingPathComponent("rollout-fixture-\(otherID).jsonl"))
    let guardianID = "10000000-0000-0000-0000-000000000004"
    try! Data((json(event("session_meta", base, ["id": guardianID, "cwd": "/fixture", "source": ["subagent": ["guardian": [:]]]])) + "\n").utf8)
        .write(to: directory.appendingPathComponent("rollout-fixture-\(guardianID).jsonl"))
    let discovered = CodexTelemetry.sessions(project: "/fixture", homeURL: home)
    check(Set(discovered.map(\.id)) == Set([parentID, childID]), "Codex catalog project/guardian filter failed")
    let discoveredParent = discovered.first { $0.id == parentID }!
    let discoveredChild = discovered.first { $0.id == childID }!
    check(discoveredChild.parentSessionID == parentID && discoveredChild.agent?.contains("Reviewer") == true, "Codex catalog parent/agent metadata lost")
    check(discoveredParent.catalogNotice?.contains("45") == true, "Codex bounded discovery coverage is not exposed")
    let collected = CodexTelemetry.collect(session: discoveredParent, sessions: discovered)
    check(collected.error == nil && collected.subagents.count == 1 && collected.toolEvents.count == 5, "Codex bounded file collection differs from parser")
    let outside = scratchURL.appendingPathComponent("rollout-outside-\(otherID).jsonl")
    try! Data((json(event("session_meta", base, ["id": otherID, "cwd": "/fixture"])) + "\n").utf8).write(to: outside)
    var unsafe = discoveredParent; unsafe.id = otherID; unsafe.url = outside
    check(CodexTelemetry.collect(session: unsafe).error?.contains("outside") == true, "Codex reader escaped its sessions directory")
    let linkID = "10000000-0000-0000-0000-000000000005"
    let link = directory.appendingPathComponent("rollout-link-\(linkID).jsonl")
    try! FileManager.default.createSymbolicLink(at: link, withDestinationURL: outside)
    unsafe.id = linkID; unsafe.url = link
    check(CodexTelemetry.collect(session: unsafe).error?.contains("symbolic link") == true, "Codex reader followed a symlink")
    check(!CodexTelemetry.sessions(project: "/fixture", homeURL: home).contains { $0.id == linkID }, "Codex catalog followed a symlink")
    let largeID = "10000000-0000-0000-0000-000000000006"
    let largeURL = directory.appendingPathComponent("rollout-large-\(largeID).jsonl")
    let largeHeader = event("session_meta", base, ["id": largeID, "cwd": "/fixture"])
    check(FileManager.default.createFile(atPath: largeURL.path, contents: Data((json(largeHeader) + "\n").utf8)), "Codex oversized fixture creation failed")
    let handle = try! FileHandle(forWritingTo: largeURL)
    try! handle.seek(toOffset: UInt64(33 * 1_024 * 1_024))
    try! handle.write(contentsOf: Data(("\n" + json(event("response_item", base, ["type": "message", "role": "user", "content": [["type": "input_text", "text": "Current task at the end"]]])) + "\n").utf8))
    try! handle.close()
    var large = discoveredParent; large.id = largeID; large.url = largeURL
    let recent = CodexTelemetry.collect(session: large)
    check(recent.error == nil && recent.taskPrompt == "Current task at the end" && recent.warnings.contains { $0.contains("earlier messages") }, "Codex oversized live rollout did not retain recent work with explicit partial coverage")
}
