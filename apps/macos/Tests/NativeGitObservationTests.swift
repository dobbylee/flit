import Foundation

enum NativeGitObservationTestFailure: Error {
    case failed(String)
}

func requireGitObservation(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw NativeGitObservationTestFailure.failed(message)
    }
}

@main
struct NativeGitObservationTests {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw NativeGitObservationTestFailure.failed(
                "expected one disposable test root argument"
            )
        }
        let testRoot = CommandLine.arguments[1]
        let dataDirectory = "\(testRoot)/data"
        let projectDirectory = "\(testRoot)/project"
        try FileManager.default.createDirectory(
            atPath: projectDirectory,
            withIntermediateDirectories: true
        )

        try initializeCore(
            dataDirectory: dataDirectory,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let registration = try projectRegisterJson(
            projectId: "packaged-git-project",
            displayName: "Packaged Git Project",
            selectedPath: projectDirectory,
            createdAt: "2026-08-04T00:00:00.000Z",
            clientProtocolVersion: flitClientProtocolVersion
        )
        let registered = try JSONDecoder().decode(
            FlitProjectRegistrationResponse.self,
            from: Data(registration.utf8)
        )
        try requireGitObservation(
            registered.status == .registered,
            "packaged Git Project registration must succeed"
        )
        let trust = try projectTrustJson(
            projectId: "packaged-git-project",
            selectedPath: projectDirectory,
            confirmedAt: "2026-08-04T00:00:01.000Z",
            clientProtocolVersion: flitClientProtocolVersion
        )
        let trusted = try JSONDecoder().decode(
            FlitProjectTrustResponse.self,
            from: Data(trust.utf8)
        )
        try requireGitObservation(
            trusted.project.trusted,
            "packaged Git Project must be trusted before observation"
        )

        let rendered = try gitObserveProjectJson(
            projectId: "packaged-git-project",
            clientProtocolVersion: flitClientProtocolVersion
        )
        let observation = try JSONDecoder().decode(
            FlitGitObservationResponse.self,
            from: Data(rendered.utf8)
        )
        guard case let .notWorktree(response) = observation else {
            throw NativeGitObservationTestFailure.failed(
                "packaged helper and installed Git must return a non-worktree receipt"
            )
        }
        try requireGitObservation(
            response.observation == .notWorktree
                && response.protocolVersion == flitClientProtocolVersion
                && response.projectId == "packaged-git-project"
                && response.reason == .notRepository,
            "packaged Git receipt must preserve the exact Project and non-repository fact"
        )
    }
}
