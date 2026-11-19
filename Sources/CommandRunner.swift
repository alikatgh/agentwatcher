import Foundation
import Darwin

private final class CommandAdmission {
    static let shared = CommandAdmission()
    private let lock = NSLock()
    private var active = 0

    func acquire() -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard active < 16 else { return false }
        active += 1
        return true
    }

    func release() {
        lock.lock(); defer { lock.unlock() }
        active -= 1
    }
}

// A child stuck in a kernel operation can defer SIGKILL. Reap it when the kernel
// releases it without holding up the caller or allocating a waiting thread.
private final class CommandReaper {
    static let shared = CommandReaper()
    private let queue = DispatchQueue(label: "AgentWatcher.command-reaper")
    private var children = Set<pid_t>()
    private var timer: DispatchSourceTimer?

    func adopt(_ child: pid_t) {
        queue.async {
            self.children.insert(child)
            guard self.timer == nil else { return }
            let timer = DispatchSource.makeTimerSource(queue: self.queue)
            timer.schedule(deadline: .now(), repeating: .milliseconds(100))
            timer.setEventHandler { [weak self] in
                guard let self else { return }
                for child in self.children {
                    var status: Int32 = 0
                    let result = waitpid(child, &status, WNOHANG)
                    if result == child || (result == -1 && errno == ECHILD) {
                        self.children.remove(child)
                        CommandAdmission.shared.release()
                    }
                }
                if self.children.isEmpty {
                    self.timer?.cancel()
                    self.timer = nil
                }
            }
            self.timer = timer
            timer.resume()
        }
    }
}

@discardableResult
func run(_ executable: String, _ arguments: [String], cwd: String? = nil,
         timeout: TimeInterval = 15) -> (code: Int32, output: String) {
    guard timeout.isFinite && timeout > 0 else {
        return (-1, "Command timeout must be a positive finite number.")
    }
    guard CommandAdmission.shared.acquire() else {
        return (-1, "Command concurrency limit reached (16 running or unreaped children).")
    }
    var reaperOwnsSlot = false
    defer { if !reaperOwnsSlot { CommandAdmission.shared.release() } }
    let path = URL(fileURLWithPath: executable).path
    var environment = ProcessInfo.processInfo.environment
    environment["PATH"] = "\(NSHomeDirectory())/.opencode/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    let values = [path] + arguments
    let environmentValues = environment.map { "\($0.key)=\($0.value)" }
    guard !(values + environmentValues + [cwd ?? ""]).contains(where: { $0.contains("\0") }) else {
        return (-1, "Command arguments cannot contain a null byte.")
    }
    func failure(_ operation: String, _ error: Int32 = errno) -> (code: Int32, output: String) {
        (-1, "\(operation): \(String(cString: strerror(error)))")
    }

    var descriptors: [Int32] = [0, 0]
    guard Darwin.pipe(&descriptors) == 0 else { return failure("Could not create command pipe") }
    // Keep the pipe separate from stdin/stdout/stderr even if one was closed by
    // the launching environment. Both ends must be closed on every return path.
    for index in descriptors.indices {
        if descriptors[index] < 3 {
            let replacement = fcntl(descriptors[index], F_DUPFD_CLOEXEC, 3)
            guard replacement != -1 else {
                let error = errno
                descriptors.forEach { Darwin.close($0) }
                return failure("Could not prepare command pipe", error)
            }
            Darwin.close(descriptors[index])
            descriptors[index] = replacement
        }
    }
    let reader = descriptors[0]
    var writer = descriptors[1]
    defer {
        Darwin.close(reader)
        if writer != -1 { Darwin.close(writer) }
    }
    guard fcntl(reader, F_SETFL, O_NONBLOCK) != -1,
          fcntl(reader, F_SETFD, FD_CLOEXEC) != -1,
          fcntl(writer, F_SETFD, FD_CLOEXEC) != -1 else {
        return failure("Could not prepare command pipe")
    }

    var actions: posix_spawn_file_actions_t?
    var attributes: posix_spawnattr_t?
    let actionsError = posix_spawn_file_actions_init(&actions)
    guard actionsError == 0 else { return failure("Could not prepare command", actionsError) }
    defer { posix_spawn_file_actions_destroy(&actions) }
    let attributesError = posix_spawnattr_init(&attributes)
    guard attributesError == 0 else { return failure("Could not prepare command", attributesError) }
    defer { posix_spawnattr_destroy(&attributes) }

    let setup = [
        posix_spawn_file_actions_adddup2(&actions, writer, STDOUT_FILENO),
        posix_spawn_file_actions_adddup2(&actions, writer, STDERR_FILENO),
        posix_spawn_file_actions_addclose(&actions, reader),
        posix_spawn_file_actions_addclose(&actions, writer),
        posix_spawnattr_setpgroup(&attributes, 0),
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETPGROUP | POSIX_SPAWN_CLOEXEC_DEFAULT))
    ]
    if let error = setup.first(where: { $0 != 0 }) { return failure("Could not prepare command", error) }
    if let cwd {
        let error = posix_spawn_file_actions_addchdir_np(&actions, cwd)
        if error != 0 { return failure("Could not set command directory", error) }
    }

    var argv = values.map { strdup($0) } + [nil]
    var envp = environmentValues.map { strdup($0) } + [nil]
    defer {
        argv.forEach { if let pointer = $0 { free(pointer) } }
        envp.forEach { if let pointer = $0 { free(pointer) } }
    }
    guard !argv.dropLast().contains(where: { $0 == nil }),
          !envp.dropLast().contains(where: { $0 == nil }) else {
        return failure("Could not prepare command", ENOMEM)
    }
    let deadline = ProcessInfo.processInfo.systemUptime + timeout
    var child: pid_t = 0
    let spawnError = argv.withUnsafeMutableBufferPointer { argv in
        envp.withUnsafeMutableBufferPointer { envp in
            posix_spawn(&child, path, &actions, &attributes, argv.baseAddress!, envp.baseAddress!)
        }
    }
    guard spawnError == 0 else { return failure("Could not launch \(executable)", spawnError) }
    Darwin.close(writer)
    writer = -1

    var terminationDeadline: TimeInterval?
    var killDeadline: TimeInterval?
    var timedOut = false
    var errorMessage: String?
    var output = Data()
    let outputLimit = 32 * 1_024 * 1_024
    var buffer = [UInt8](repeating: 0, count: 65_536)
    var pipeClosed = false
    var exited = false

    while true {
        // WNOWAIT reserves this child's PID until cleanup; it cannot be reused
        // between observing exit and signaling its owned process group.
        var information = siginfo_t()
        if waitid(P_PID, id_t(child), &information, WEXITED | WNOHANG | WNOWAIT) == 0 {
            exited = information.si_pid == child
        } else if errno != EINTR {
            let error = errno
            errorMessage = failure("Could not observe command exit", error).output
            // ECHILD means another waiter took ownership; never signal that PID.
            if error == ECHILD { break }
        }

        // Limit each drain pass so continuous output cannot starve the deadline.
        for _ in 0..<16 where !pipeClosed {
            let count = Darwin.read(reader, &buffer, buffer.count)
            if count > 0 {
                let retainedCount = min(count, outputLimit - output.count)
                output.append(contentsOf: buffer.prefix(retainedCount))
                if retainedCount < count {
                    errorMessage = "Command output exceeded 32 MiB capture limit: \(executable)"
                }
            }
            else if count == 0 { pipeClosed = true; break }
            else if errno == EINTR { continue }
            else if errno == EAGAIN { break }
            else { errorMessage = failure("Could not read command output").output; break }
            if ProcessInfo.processInfo.systemUptime >= (terminationDeadline ?? deadline) { break }
        }

        let now = ProcessInfo.processInfo.systemUptime
        if terminationDeadline == nil && (now >= deadline || errorMessage != nil) {
            timedOut = errorMessage == nil
            Darwin.kill(-child, SIGTERM)
            terminationDeadline = now + 0.2
        }
        if let terminationDeadline, now >= terminationDeadline, killDeadline == nil {
            // Send KILL before reaping, including when descendants kept the pipe
            // open after the direct child exited or ignored TERM.
            Darwin.kill(-child, SIGKILL)
            killDeadline = now + 0.2
        }
        if let killDeadline {
            if exited || now >= killDeadline { break }
        } else if terminationDeadline == nil && exited && pipeClosed {
            break
        }

        let nextDeadline = killDeadline ?? terminationDeadline ?? deadline
        let milliseconds = Int32(max(1, min(20, (nextDeadline - now) * 1_000)))
        var descriptor = pollfd(fd: reader, events: Int16(POLLIN | POLLHUP), revents: 0)
        let pollResult = pipeClosed ? poll(nil, 0, milliseconds) : poll(&descriptor, 1, milliseconds)
        if pollResult == -1 && errno != EINTR { errorMessage = failure("Could not wait for command output").output }
    }

    var status: Int32 = 0
    var result: pid_t
    repeat { result = waitpid(child, &status, WNOHANG) } while result == -1 && errno == EINTR
    let reapError = result == -1 ? errno : nil
    if result == 0 || (result == -1 && reapError != ECHILD) {
        reaperOwnsSlot = true
        CommandReaper.shared.adopt(child)
    }
    var text = String(decoding: output, as: UTF8.self)
    if let errorMessage { return (-1, text.isEmpty ? errorMessage : text + "\n" + errorMessage) }
    if timedOut {
        if !text.isEmpty && !text.hasSuffix("\n") { text += "\n" }
        text += "Command timed out after \(timeout) seconds: \(executable)"
        return (124, text)
    }
    guard result == child else { return failure("Could not reap command", reapError ?? EAGAIN) }
    let signal = status & 0x7f
    return (signal == 0 ? (status >> 8) & 0xff : 128 + signal, text)
}
