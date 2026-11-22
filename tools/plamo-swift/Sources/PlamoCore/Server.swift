import Dispatch
import Foundation
import Network

/// The HTTP server, on Network.framework: no raw sockets, no unsafe pointers, and every read and
/// write is a checked continuation. It runs on its own serial queue, so a decode never occupies a
/// cooperative-pool thread.
public actor PlamoServer {
    /// Where the server listens. The loopback port stays for the clients that only speak TCP; the
    /// Unix socket is the cheaper path for the ones that can use it.
    public enum Endpoint: Sendable, Equatable {
        case tcp(UInt16)
        case unix(String)

        public var description: String {
            switch self {
            case .tcp(let port):
                return "127.0.0.1:\(port)"
            case .unix(let path):
                return path
            }
        }
    }

    public struct Configuration: Sendable, Equatable {
        public var port: UInt16?
        public var socketPath: String?
        public var idleSeconds: Double
        /// Idle time after which the weights are released; 0 keeps them resident.
        public var lukewarmSeconds: Double
        public var idleCheckInterval: Double

        public init(
            port: UInt16? = Invocation.defaultPort,
            socketPath: String? = nil,
            idleSeconds: Double = Invocation.defaultIdleSeconds,
            lukewarmSeconds: Double = Invocation.defaultLukewarmSeconds,
            idleCheckInterval: Double = 5
        ) {
            self.port = port
            self.socketPath = socketPath
            self.idleSeconds = idleSeconds
            self.lukewarmSeconds = lukewarmSeconds
            self.idleCheckInterval = idleCheckInterval
        }

        /// Every endpoint to listen on, in the order they are started.
        public var endpoints: [Endpoint] {
            var endpoints: [Endpoint] = []
            if let port { endpoints.append(.tcp(port)) }
            if let socketPath { endpoints.append(.unix(socketPath)) }
            return endpoints
        }
    }

    private static let maximumChunk = 65_536

    private let router: Router
    private let configuration: Configuration
    private let onIdle: @Sendable () -> Void
    private let onLukewarm: @Sendable () async -> Void
    private let queue = DispatchSerialQueue(label: "plamo-swift.server")
    private var listeners: [NWListener] = []
    private var idle = IdleTracker()
    private var idleTask: Task<Void, Never>?
    private var lukewarm = false
    private var stopped: CheckedContinuation<Void, Never>?
    private var running = true
    private var readyWaiter: CheckedContinuation<Void, any Error>?
    private var bound: [String] = []
    private var createdSocketPath: String?

    /// The server runs on its own queue, so every hop into it is a real thread hop.
    nonisolated public var unownedExecutor: UnownedSerialExecutor {
        queue.asUnownedSerialExecutor()
    }

    /// `onIdle` is where the process ends: production passes `exit(0)`, the tests count the calls.
    /// `onLukewarm` releases the weights after a shorter idle and is where memory goes back.
    public init(
        router: Router,
        configuration: Configuration = Configuration(),
        onLukewarm: @escaping @Sendable () async -> Void = {},
        onIdle: @escaping @Sendable () -> Void = { exit(0) }
    ) {
        self.router = router
        self.configuration = configuration
        self.onLukewarm = onLukewarm
        self.onIdle = onIdle
    }

    /// Starts every configured endpoint and returns where it listens, for the log line.
    public func start() async throws -> String {
        guard listeners.isEmpty else {
            throw PlamoError.listener(
                port: configuration.port ?? 0, reason: "already started")
        }
        let endpoints = configuration.endpoints
        guard !endpoints.isEmpty else {
            throw PlamoError.listener(port: 0, reason: "no endpoint configured")
        }
        do {
            for endpoint in endpoints {
                try startListener(for: endpoint)
                try await waitUntilReady()
            }
        } catch {
            stopListeners()
            throw error
        }
        startIdleWatch()
        return bound.joined(separator: " and ")
    }

    /// Suspends until the listener just started is bound. Endpoints start one at a time: a second
    /// `serve` must fail on the port before it unlinks the socket of the server that owns it.
    private func waitUntilReady() async throws {
        try await withCheckedThrowingContinuation { continuation in
            readyWaiter = continuation
        }
    }

    /// Stops the listeners and resumes `waitUntilStopped`.
    public func stop() {
        running = false
        idleTask?.cancel()
        idleTask = nil
        stopped?.resume()
        stopped = nil
        stopListeners()
        readyWaiter?.resume(throwing: PlamoError.connectionClosed)
        readyWaiter = nil
    }

    private func stopListeners() {
        for listener in listeners {
            listener.cancel()
        }
        listeners.removeAll()
        bound.removeAll()
        // Only a path this server created may be removed: a failed start must leave the socket of
        // the server that already owns the port alone.
        if let path = createdSocketPath {
            try? FileManager.default.removeItem(atPath: path)
            createdSocketPath = nil
        }
    }

    private func startListener(for endpoint: Endpoint) throws {
        let parameters = NWParameters.tcp
        parameters.allowLocalEndpointReuse = true
        let listener: NWListener
        do {
            switch endpoint {
            case .tcp(let port):
                parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
                listener = try NWListener(
                    using: parameters, on: NWEndpoint.Port(rawValue: port) ?? .any)
            case .unix(let path):
                // A crash leaves the file behind, and bind refuses an existing path.
                try? FileManager.default.removeItem(atPath: path)
                createdSocketPath = path
                parameters.requiredLocalEndpoint = .unix(path: path)
                listener = try NWListener(using: parameters)
            }
        } catch {
            throw PlamoError.listener(
                port: configuration.port ?? 0, reason: "\(endpoint.description): \(error)")
        }
        listener.newConnectionHandler = { [weak self] connection in
            Task { await self?.accept(connection) }
        }
        listener.stateUpdateHandler = { [weak self, weak listener] state in
            let port = listener?.port?.rawValue
            Task { await self?.listenerChanged(state, endpoint: endpoint, port: port) }
        }
        listeners.append(listener)
        listener.start(queue: queue)
    }

    /// One request per connection, unless the client asks to keep it: the mods' agent reuses the
    /// socket for every translation, so only the first one pays the connect.
    private func accept(_ connection: NWConnection) {
        connection.start(queue: queue)
        Task { await self.serve(connection) }
    }

    private func serve(_ connection: NWConnection) async {
        defer { connection.cancel() }
        var buffer = Data()
        do {
            while true {
                let request = try await readRequest(connection, buffer: &buffer)
                Timing.mark("req.read")
                idle.touch()
                // The runtime reloads on its own; this only stops the watcher from releasing the
                // weights again while the request is in flight.
                lukewarm = false
                idle.setBusy(true)
                let response = await router.respond(to: request)
                idle.setBusy(false)
                idle.touch()
                try await send(connection, response.serialized(keepAlive: request.keepAlive))
                if !request.keepAlive { return }
            }
        } catch PlamoError.connectionClosed {
            return
        } catch {
            let response = HTTPResponse(
                status: "400 Bad Request",
                body: #"{"error":{"message":"bad request","type":"invalid_request_error"}}"#)
            try? await send(connection, response.serialized())
        }
    }

    private func readRequest(
        _ connection: NWConnection, buffer: inout Data
    ) async throws
        -> HTTPRequest
    {
        while HTTPRequestParser.headerEnd(in: buffer) == nil {
            guard let chunk = try await receive(connection) else {
                throw PlamoError.connectionClosed
            }
            buffer.append(chunk)
            guard buffer.count <= HTTPRequestParser.maximumBodyBytes else {
                throw PlamoError.invalidRequest(reason: "request too large")
            }
        }
        guard let end = HTTPRequestParser.headerEnd(in: buffer) else {
            throw PlamoError.invalidRequest(reason: "request head")
        }
        let head = Data(buffer.prefix(end))
        let length = try HTTPRequestParser.contentLength(head: head)
        while buffer.count - end < length {
            guard let chunk = try await receive(connection) else {
                throw PlamoError.connectionClosed
            }
            buffer.append(chunk)
        }
        let body = Data(buffer.dropFirst(end).prefix(length))
        buffer = Data(buffer.dropFirst(end + length))
        return try HTTPRequestParser.parse(head: head, body: body)
    }

    /// nil means the peer closed without sending anything more.
    private func receive(_ connection: NWConnection) async throws -> Data? {
        try await withCheckedThrowingContinuation { continuation in
            connection.receive(minimumIncompleteLength: 1, maximumLength: Self.maximumChunk) {
                data, _, isComplete, error in
                if error != nil {
                    continuation.resume(returning: nil)
                } else if let data, !data.isEmpty {
                    continuation.resume(returning: data)
                } else if isComplete {
                    continuation.resume(returning: nil)
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }

    private func send(_ connection: NWConnection, _ data: Data) async throws {
        try await withCheckedThrowingContinuation { continuation in
            connection.send(
                content: data,
                completion: .contentProcessed { error in
                    if error == nil {
                        continuation.resume()
                    } else {
                        continuation.resume(throwing: PlamoError.connectionClosed)
                    }
                })
        }
    }

    private func listenerChanged(_ state: NWListener.State, endpoint: Endpoint, port: UInt16?) {
        switch state {
        case .ready:
            bound.append(describe(endpoint, port: port))
            if case .unix(let path) = endpoint {
                // The socket is the server's own: nobody else may talk to it.
                try? FileManager.default.setAttributes(
                    [.posixPermissions: 0o600], ofItemAtPath: path)
            }
            readyWaiter?.resume()
            readyWaiter = nil
        case .failed(let error):
            readyWaiter?.resume(
                throwing: PlamoError.listener(
                    port: configuration.port ?? 0,
                    reason: "\(endpoint.description): \(error)"))
            readyWaiter = nil
        default:
            break
        }
    }

    private func describe(_ endpoint: Endpoint, port: UInt16?) -> String {
        if case .tcp = endpoint, let port {
            return Endpoint.tcp(port).description
        }
        return endpoint.description
    }

    /// Suspends until `stop()`. The idle timer is what normally ends it, so the process does not
    /// outlive its last request — the tests pass their own `onIdle` and resume it directly.
    public func waitUntilStopped() async {
        guard running else { return }
        await withCheckedContinuation { continuation in
            stopped = continuation
        }
    }

    /// Exits only after a full idle interval with no request in flight.
    private func startIdleWatch() {
        guard idleTask == nil,
            configuration.idleSeconds > 0 || configuration.lukewarmSeconds > 0
        else { return }
        let interval = configuration.idleCheckInterval
        idleTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(interval))
                guard let self else { return }
                // Keep watching while the server is busy or recently used; only the expiry ends it.
                if await self.idleExpired() { return }
                await self.enterLukewarmIfIdle()
            }
        }
    }

    private func idleExpired() -> Bool {
        guard configuration.idleSeconds > 0,
            idle.shouldExit(now: Date(), idleSeconds: configuration.idleSeconds)
        else { return false }
        onIdle()
        return true
    }

    /// Releases the weights after a shorter idle than the exit: the process stays, the memory goes,
    /// and the next request reads the weights again instead of starting a process.
    private func enterLukewarmIfIdle() async {
        guard !lukewarm, configuration.lukewarmSeconds > 0, !idle.busy,
            Date().timeIntervalSince(idle.lastActivity) > configuration.lukewarmSeconds
        else { return }
        lukewarm = true
        await onLukewarm()
    }
}
