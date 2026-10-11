import Foundation

enum APIError: LocalizedError {
    case server(status: Int, message: String)
    case transport(Error)
    case decoding(Error)

    var errorDescription: String? {
        switch self {
        case .server(_, let message): message
        case .transport(let error): "Cannot reach the mail server: \(error.localizedDescription)"
        case .decoding(let error): "Unexpected response from server: \(error.localizedDescription)"
        }
    }
}

/// Thin async client for the mymail-server HTTP API (see ARCHITECTURE.md).
struct APIClient {
    static let defaultBaseURL = URL(string: "http://127.0.0.1:7878")!

    let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = APIClient.defaultBaseURL) {
        self.baseURL = baseURL
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 10
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        self.session = URLSession(configuration: config)
    }

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        d.dateDecodingStrategy = .secondsSince1970
        return d
    }()

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        e.dateEncodingStrategy = .secondsSince1970
        return e
    }()

    private struct ErrorBody: Decodable { let error: String }
    private struct Empty: Encodable {}

    // MARK: - Core

    private func request(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        body: (some Encodable)? = Empty?.none
    ) async throws -> Data {
        var components = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        if !query.isEmpty { components.queryItems = query }
        var req = URLRequest(url: components.url!)
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            req.httpBody = try Self.encoder.encode(body)
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: req)
        } catch {
            throw APIError.transport(error)
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let message = (try? Self.decoder.decode(ErrorBody.self, from: data))?.error
                ?? HTTPURLResponse.localizedString(forStatusCode: status).capitalized
            throw APIError.server(status: status, message: message)
        }
        return data
    }

    private func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try Self.decoder.decode(type, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    private func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        try decode(T.self, from: try await request("GET", path, query: query))
    }

    private func send<T: Decodable>(_ method: String, _ path: String, body: (some Encodable)? = Empty?.none) async throws -> T {
        try decode(T.self, from: try await request(method, path, body: body))
    }

    // MARK: - Endpoints

    func health() async -> Bool {
        struct Health: Decodable { let status: String }
        let h: Health? = try? await get("api/health")
        return h?.status == "ok"
    }

    func accounts() async throws -> [Account] {
        try await get("api/accounts")
    }

    func createAccount(name: String, email: String) async throws -> Account {
        try await send("POST", "api/accounts", body: NewAccount(name: name, email: email))
    }

    func deleteAccount(_ id: Int64) async throws {
        _ = try await request("DELETE", "api/accounts/\(id)")
    }

    func folderCounts(accountId: Int64) async throws -> [FolderCount] {
        try await get("api/accounts/\(accountId)/folders")
    }

    @discardableResult
    func markAllRead(accountId: Int64, folder: Folder) async throws -> Int {
        struct Updated: Decodable { let updated: Int }
        let r: Updated = try await send("POST", "api/accounts/\(accountId)/folders/\(folder.rawValue)/read")
        return r.updated
    }

    @discardableResult
    func emptyTrash(accountId: Int64) async throws -> Int {
        struct Deleted: Decodable { let deleted: Int }
        let r: Deleted = try await send("POST", "api/accounts/\(accountId)/trash/empty")
        return r.deleted
    }

    func messages(accountId: Int64, folder: Folder, query: String?) async throws -> [MessageSummary] {
        var items = [
            URLQueryItem(name: "account_id", value: String(accountId)),
            URLQueryItem(name: "folder", value: folder.rawValue),
        ]
        if let query, !query.trimmingCharacters(in: .whitespaces).isEmpty {
            items.append(URLQueryItem(name: "q", value: query))
        }
        return try await get("api/messages", query: items)
    }

    func message(_ id: Int64) async throws -> Message {
        try await get("api/messages/\(id)")
    }

    @discardableResult
    func updateMessage(_ id: Int64, _ patch: MessagePatch) async throws -> Message {
        try await send("PATCH", "api/messages/\(id)", body: patch)
    }

    /// Returns true when the message was permanently deleted.
    @discardableResult
    func deleteMessage(_ id: Int64) async throws -> Bool {
        struct Deleted: Decodable { let permanent: Bool }
        let r: Deleted = try await send("DELETE", "api/messages/\(id)")
        return r.permanent
    }

    func sendMessage(_ compose: Compose) async throws -> SendResult {
        try await send("POST", "api/send", body: compose)
    }

    func saveDraft(_ compose: Compose, id: Int64?) async throws -> Message {
        if let id {
            return try await send("PUT", "api/drafts/\(id)", body: compose)
        }
        return try await send("POST", "api/drafts", body: compose)
    }
}
