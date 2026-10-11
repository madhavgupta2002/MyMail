import Foundation

/// Ensures a mymail-server is reachable, spawning a bundled/local one if needed,
/// and stops any server it started when the app quits.
@MainActor
final class ServerManager {
    static let shared = ServerManager()

    private var process: Process?
    private let api = APIClient()

    enum StartError: LocalizedError {
        case binaryNotFound([String])
        case launchFailed(String)
        case timedOut(String?)

        var errorDescription: String? {
            switch self {
            case .binaryNotFound(let searched):
                "Could not find the mymail-server binary. Looked in:\n" + searched.joined(separator: "\n")
            case .launchFailed(let reason):
                "Failed to start mymail-server: \(reason)"
            case .timedOut(let output):
                "mymail-server did not become healthy in time." + (output.map { "\n\($0)" } ?? "")
            }
        }
    }

    /// Returns once the server answers `GET /api/health`.
    func ensureRunning() async throws {
        if await api.health() { return }
        if process?.isRunning != true {
            try launch()
        }
        for _ in 0..<50 {
            if await api.health() { return }
            if let process, !process.isRunning {
                throw StartError.launchFailed("server exited with status \(process.terminationStatus)\n\(readLog() ?? "")")
            }
            try await Task.sleep(for: .milliseconds(200))
        }
        throw StartError.timedOut(readLog())
    }

    func stop() {
        guard let process, process.isRunning else { return }
        process.terminate()
        // Give it a moment for a clean shutdown (WAL checkpoint), then force.
        let deadline = Date().addingTimeInterval(2)
        while process.isRunning && Date() < deadline {
            usleep(50_000)
        }
        if process.isRunning {
            kill(process.processIdentifier, SIGKILL)
        }
        self.process = nil
    }

    // MARK: - Private

    private var logURL: URL { dataDirectory.appendingPathComponent("server.log") }

    var dataDirectory: URL {
        let env = ProcessInfo.processInfo.environment
        if let dir = env["MYMAIL_DATA_DIR"], !dir.isEmpty {
            return URL(fileURLWithPath: dir, isDirectory: true)
        }
        if let dir = Bundle.main.object(forInfoDictionaryKey: "MyMailDataDir") as? String, !dir.isEmpty {
            return URL(fileURLWithPath: dir, isDirectory: true)
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent("MyMail", isDirectory: true)
    }

    private func candidateBinaries() -> [String] {
        var paths: [String] = []
        if let bin = ProcessInfo.processInfo.environment["MYMAIL_SERVER_BIN"], !bin.isEmpty {
            paths.append(bin)
        }
        if let bundled = Bundle.main.url(forResource: "mymail-server", withExtension: nil) {
            paths.append(bundled.path)
        }
        let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        paths.append(cwd.appendingPathComponent("../backend/target/release/mymail-server").standardized.path)
        return paths
    }

    private func launch() throws {
        let candidates = candidateBinaries()
        guard let binary = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            throw StartError.binaryNotFound(candidates)
        }

        let dataDir = dataDirectory
        try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        let log = try FileHandle(forWritingTo: logURL)

        let p = Process()
        p.executableURL = URL(fileURLWithPath: binary)
        p.arguments = ["--db", dataDir.appendingPathComponent("mymail.db").path]
        p.currentDirectoryURL = dataDir
        p.standardOutput = log
        p.standardError = log
        do {
            try p.run()
        } catch {
            throw StartError.launchFailed(error.localizedDescription)
        }
        process = p
    }

    private func readLog() -> String? {
        guard let data = try? Data(contentsOf: logURL), !data.isEmpty,
              let text = String(data: data, encoding: .utf8) else { return nil }
        return text.split(separator: "\n").suffix(10).joined(separator: "\n")
    }
}
