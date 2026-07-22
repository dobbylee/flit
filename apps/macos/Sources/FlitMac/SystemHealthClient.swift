import Foundation

enum HealthStatus: String, Decodable, Sendable {
    case ready
    case notConfigured = "not_configured"
    case unavailable
}

struct SystemHealthPayload: Decodable, Equatable, Sendable {
    let protocolVersion: String
    let core: HealthStatus
    let storage: HealthStatus
    let providers: HealthStatus

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case core
        case storage
        case providers
    }
}

struct CommandFailurePayload: Decodable, Equatable, Sendable {
    let code: String
    let messageKey: String

    enum CodingKeys: String, CodingKey {
        case code
        case messageKey = "message_key"
    }
}

enum FoundationHealth: Equatable, Sendable {
    case ready(SystemHealthPayload)
    case unavailable(messageKey: String?)
}

struct SystemHealthClient: Sendable {
    let clientProtocolVersion: String

    init(clientProtocolVersion: String = flitClientProtocolVersion) {
        self.clientProtocolVersion = clientProtocolVersion
    }

    func load() -> FoundationHealth {
        do {
            let rendered = try systemHealthJson(clientProtocolVersion: clientProtocolVersion)
            let data = Data(rendered.utf8)

            if let health = try? JSONDecoder().decode(SystemHealthPayload.self, from: data),
                health.protocolVersion == clientProtocolVersion,
                health.core == .ready,
                health.storage == .notConfigured,
                health.providers == .notConfigured
            {
                return .ready(health)
            }

            let failure = try? JSONDecoder().decode(CommandFailurePayload.self, from: data)
            return .unavailable(messageKey: failure?.messageKey)
        } catch {
            return .unavailable(messageKey: nil)
        }
    }
}
