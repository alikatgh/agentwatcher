import Foundation
import Darwin

@main
enum CommandRunnerChecks {
    static func check(_ condition: @autoclosure () -> Bool, _ message: String) {
        guard condition() else { fatalError("FAIL: " + message) }
    }

    static func main() {
        let large = run("/usr/bin/head", ["-c", "2097152", "/dev/zero"], timeout: 3)
        check(large.code == 0 && large.output.utf8.count == 2_097_152, "large output deadlocked or was truncated")
        let nonzero = run("/bin/sh", ["-c", "printf stdout; printf stderr >&2; exit 17"])
        check(nonzero.code == 17 && nonzero.output == "stdoutstderr", "nonzero status or merged output lost")
        let signaled = run("/bin/sh", ["-c", "kill -HUP $$"])
        check(signaled.code == 128 + SIGHUP, "SIGHUP collided with ordinary exit status 1")
        check(run("/bin/sh", ["-c", "exit 1"]).code == 1, "ordinary exit status 1 changed")
        let environment = run("/bin/sh", ["-c", "printf '%s\\n' \"$PATH\"; pwd"], cwd: "/")
        check(environment.code == 0 && environment.output == "\(NSHomeDirectory())/.opencode/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin\n/\n", "PATH or working directory changed")
        print("PASS: output beyond pipe capacity, merged errors/nonzero/signal status, PATH and cwd")

        do {
            let began = ProcessInfo.processInfo.systemUptime
            let excessive = run("/bin/sh", ["-c", "printf '%s\\n' $$; /usr/bin/head -c 41943040 /dev/zero; exec /bin/sleep 20"], timeout: 5)
            let pid = pid_t(excessive.output.split(separator: "\n")[0])!
            check(excessive.code != 0 && excessive.output.contains("Command output exceeded 32 MiB capture limit"), "oversized output became successful truncated data")
            check(excessive.output.utf8.count <= 32 * 1_024 * 1_024 + 256, "output capture exceeded its limit plus diagnostic")
            check(ProcessInfo.processInfo.systemUptime - began < 2, "output limit did not stop the owned command promptly")
            check(Darwin.kill(pid, 0) == -1 && errno == ESRCH, "output-limited owned child still exists")
        }
        print("PASS: output above 32 MiB fails explicitly, stops and reaps the owned command")

        let ready = DispatchGroup()
        let finished = DispatchGroup()
        let start = DispatchSemaphore(value: 0)
        let admissionLock = NSLock()
        var admissionResults: [(code: Int32, output: String)] = []
        for _ in 0..<17 {
            ready.enter()
            finished.enter()
            Thread.detachNewThread {
                ready.leave()
                start.wait()
                let result = run("/bin/sleep", ["1"], timeout: 3)
                admissionLock.lock(); admissionResults.append(result); admissionLock.unlock()
                finished.leave()
            }
        }
        check(ready.wait(timeout: .now() + 5) == .success, "admission fixture threads did not start")
        for _ in 0..<17 { start.signal() }
        check(finished.wait(timeout: .now() + 5) == .success, "admission fixture commands did not finish")
        let rejected = admissionResults.filter { $0.code != 0 }
        check(!rejected.isEmpty && rejected.allSatisfy { $0.output.contains("Command concurrency limit reached") }, "concurrency admission accepted all 17 owned children")
        print("PASS: at most 16 owned children admitted; excess commands fail explicitly")

        for script in ["printf '%s\\n' $$; exec /bin/sleep 20", "trap '' TERM; printf '%s\\n' $$; exec /bin/sleep 20"] {
            let began = ProcessInfo.processInfo.systemUptime
            let result = run("/bin/sh", ["-c", script], timeout: 0.15)
            let elapsed = ProcessInfo.processInfo.systemUptime - began
            let pid = pid_t(result.output.split(separator: "\n")[0])!
            check(result.code == 124 && result.output.contains("timed out"), "sleep timeout was reported as success")
            check(elapsed < 0.9, "sleep exceeded timeout plus kill grace: \(elapsed)s")
            check(Darwin.kill(pid, 0) == -1 && errno == ESRCH, "timed-out owned child still exists")
            print("PASS: sleeping owned child timed out and reaped in \(String(format: "%.3f", elapsed))s")
        }
        let descendantBegan = ProcessInfo.processInfo.systemUptime
        let descendant = run("/bin/sh", ["-c", "/bin/sleep 20 & printf '%s\\n' $!; exit 0"], timeout: 0.15)
        check(descendant.code == 124 && ProcessInfo.processInfo.systemUptime - descendantBegan < 0.9, "inherited pipe defeated deadline")
        let descendantPID = descendant.output.split(separator: "\n")[0]
        var descendantGone = false
        let descendantExitDeadline = ProcessInfo.processInfo.systemUptime + 0.5
        repeat {
            descendantGone = Darwin.kill(pid_t(descendantPID)!, 0) == -1 && errno == ESRCH
            if descendantGone { break }
            Thread.sleep(forTimeInterval: 0.01)
        } while ProcessInfo.processInfo.systemUptime < descendantExitDeadline
        check(descendantGone, "timed-out descendant still exists")
        print("PASS: descendant-held output pipe cannot defeat deadline")

        let continuousBegan = ProcessInfo.processInfo.systemUptime
        let continuous = run("/bin/sh", ["-c", "while :; do printf 'still writing\\n'; done"], timeout: 0.15)
        check(continuous.code == 124 && !continuous.output.isEmpty && ProcessInfo.processInfo.systemUptime - continuousBegan < 0.9, "continuous output defeated deadline")
        print("PASS: continuous output cannot starve timeout")

        func descriptors() -> Int { (0..<256).filter { fcntl(Int32($0), F_GETFD) != -1 }.count }
        let before = descriptors()
        for index in 0..<64 {
            let result = run("/bin/echo", ["fixture-\(index)"])
            check(result.code == 0 && result.output == "fixture-\(index)\n", "repeated commands mixed outputs")
        }
        let lock = NSLock()
        var failures: [Int] = []
        DispatchQueue.concurrentPerform(iterations: 8) { index in
            let result = run("/bin/echo", ["parallel-\(index)"])
            if result.code != 0 || result.output != "parallel-\(index)\n" {
                lock.lock(); failures.append(index); lock.unlock()
            }
        }
        check(failures.isEmpty, "parallel commands mixed outputs")
        check(descriptors() == before, "commands leaked file descriptors")
        let missing = run("/nonexistent-agentwatcher-command-fixture", [])
        check(missing.code != 0 && missing.output.contains("Could not launch"), "launch failure was reported as success")
        check(run("/bin/echo", ["invalid\0argument"]).code != 0, "null byte silently changed an argument")
        print("PASS: 64 sequential and 8 parallel commands, stable descriptor count, launch/argument errors")
    }
}
