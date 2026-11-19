import Foundation
import Darwin

// Start Git outside the worktree so its initial getcwd does not walk the
// project's parent directories. Explicit routing is limited to owned roots;
// discovery remains available for subfolders, bare repos and other owners.
func repositoryCommand(_ project: String, _ arguments: [String]) -> (code: Int32, output: String) {
    let root = URL(fileURLWithPath: project, isDirectory: true).standardizedFileURL
    let metadata = root.appendingPathComponent(".git")
    let fm = FileManager.default
    func owned(_ url: URL) -> Bool {
        // Foundation's full attribute dictionary also fetches extended
        // attributes, which can block on managed Documents folders.
        var attributes = stat()
        return url.path.withCString { fstatat(AT_FDCWD, $0, &attributes, 0) == 0 } && attributes.st_uid == getuid()
    }
    var gitDirectory = metadata
    var isDirectory: ObjCBool = false
    let exists = fm.fileExists(atPath: metadata.path, isDirectory: &isDirectory)
    var mayRouteExplicitly = exists && owned(root) && owned(metadata)
    if mayRouteExplicitly && !isDirectory.boolValue {
        if let pointer = try? String(contentsOf: metadata, encoding: .utf8), pointer.hasPrefix("gitdir: ") {
            let target = String(pointer.dropFirst(8)).trimmingCharacters(in: .whitespacesAndNewlines)
            gitDirectory = URL(fileURLWithPath: target, isDirectory: true, relativeTo: root).standardizedFileURL
            mayRouteExplicitly = !target.isEmpty && owned(gitDirectory)
        } else {
            mayRouteExplicitly = false
        }
    }
    let commonFile = gitDirectory.appendingPathComponent("commondir")
    if mayRouteExplicitly && fm.fileExists(atPath: commonFile.path) {
        if let common = try? String(contentsOf: commonFile, encoding: .utf8) {
            let target = common.trimmingCharacters(in: .whitespacesAndNewlines)
            let base = URL(fileURLWithPath: gitDirectory.path, isDirectory: true)
            mayRouteExplicitly = owned(commonFile) && !target.isEmpty && owned(URL(fileURLWithPath: target, isDirectory: true, relativeTo: base).standardizedFileURL)
        } else {
            mayRouteExplicitly = false
        }
    }
    let routing = mayRouteExplicitly
        ? ["--git-dir=\(metadata.path)", "--work-tree=\(root.path)"]
        : ["-C", root.path]
    return run("/usr/bin/git", ["--no-optional-locks"] + routing + arguments, cwd: "/")
}
