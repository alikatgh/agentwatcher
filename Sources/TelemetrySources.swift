import Foundation

enum TelemetrySources {
    struct Request: Equatable {
        var project: String
        var logFile: String
        var selection: String
    }
    struct Result {
        var snapshot: TelemetrySnapshot
        var sessions: [DeepSeekTelemetry.SessionDescriptor]
        var codexSessions: [CodexTelemetry.SessionDescriptor]
    }

    static func collect(_ request: Request) -> Result {
        let sessions = DeepSeekTelemetry.sessions()
        let codexSessions = CodexTelemetry.sessions(project: request.project)
        let snapshot: TelemetrySnapshot
        if ["automatic", "codex"].contains(request.selection),
           let session = codexSessions.first(where: { $0.parentSessionID == nil }) ?? codexSessions.first {
            snapshot = CodexTelemetry.collect(session: session, sessions: codexSessions)
        } else if request.selection.hasPrefix("codex:"),
                  let session = codexSessions.first(where: { $0.id == String(request.selection.dropFirst(6)) }) {
            snapshot = CodexTelemetry.collect(session: session, sessions: codexSessions)
        } else if request.selection == "automatic", let session = sessions.first {
            snapshot = DeepSeekTelemetry.collect(session: session)
        } else if let session = sessions.first(where: { $0.url.path == request.selection || $0.id == request.selection }) {
            snapshot = DeepSeekTelemetry.collect(session: session)
        } else if request.selection.hasPrefix("opencode:") {
            snapshot = AgentTelemetry.collect(project: request.project, sessionID: String(request.selection.dropFirst(9)))
        } else if ["automatic", "opencode"].contains(request.selection) {
            snapshot = AgentTelemetry.collect(project: request.project, logFile: request.logFile)
        } else {
            snapshot = TelemetrySnapshot(sessionID: "", title: "Session unavailable", project: request.project,
                                         collectedAt: Date(), error: "The selected recorded session could not be found.")
        }
        return Result(snapshot: snapshot, sessions: sessions, codexSessions: codexSessions)
    }
}
