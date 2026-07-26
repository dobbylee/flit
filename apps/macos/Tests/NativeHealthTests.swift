import AppKit
import Foundation

enum NativeHealthTestFailure: Error {
    case failed(String)
}

@MainActor
private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw NativeHealthTestFailure.failed(message)
    }
}

private func canonicalJSON(at path: String) throws -> Data {
    let object = try JSONSerialization.jsonObject(
        with: Data(contentsOf: URL(fileURLWithPath: path))
    )
    return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
}

private func canonicalJSON(from text: String) throws -> Data {
    let object = try JSONSerialization.jsonObject(with: Data(text.utf8))
    return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
}

private func decodeFixture<T: Decodable>(_ type: T.Type, at path: String) throws -> T {
    try JSONDecoder().decode(type, from: Data(contentsOf: URL(fileURLWithPath: path)))
}

@MainActor
private func requireDecodingFailure<T: Decodable>(
    _ type: T.Type,
    from data: Data,
    _ message: String
) throws {
    do {
        _ = try JSONDecoder().decode(type, from: data)
    } catch {
        return
    }
    throw NativeHealthTestFailure.failed(message)
}

@MainActor
private func requireCommandError(
    _ text: String,
    code: FlitCommandErrorCode,
    fixtures: [FlitCommandError]
) throws {
    let actual = try JSONDecoder().decode(FlitCommandError.self, from: Data(text.utf8))
    guard let expected = fixtures.first(where: { $0.code == code }) else {
        throw NativeHealthTestFailure.failed("missing fixture for command error \(code)")
    }
    try require(actual == expected, "real command error \(code) must match its fixture")
}

@MainActor
private func requireFoundationLayout(
    _ controller: FoundationViewController,
    in window: NSWindow,
    size: NSSize,
    expectedPanelWidth: CGFloat
) throws {
    window.setContentSize(size)
    window.contentView?.needsLayout = true
    window.contentView?.layoutSubtreeIfNeeded()
    guard let panelFrame = controller.foundationPanelFrame else {
        throw NativeHealthTestFailure.failed("foundation panel must be available after layout")
    }
    try require(
        !controller.hasAmbiguousFoundationLayout,
        "foundation layout must not be ambiguous at width \(size.width)"
    )
    try require(
        abs(panelFrame.width - expectedPanelWidth) < 0.5,
        "foundation panel width \(panelFrame.width) must be \(expectedPanelWidth) at requested width \(size.width), actual width \(controller.view.bounds.width)"
    )
    try require(
        panelFrame.minX >= 48 && panelFrame.maxX <= size.width - 48,
        "foundation panel must preserve 48-point side margins"
    )
}

@main
@MainActor
struct NativeHealthTests {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw NativeHealthTestFailure.failed("expected repository root argument")
        }
        let root = CommandLine.arguments[1]
        let fixtureRoot =
            "\(root)/fixtures/protocol/commands/v\(flitClientProtocolVersion)"
        let dataDirectory = "\(root)/target/flit-macos/native-health-data"

        let requestData = try canonicalJSON(
            at: "\(fixtureRoot)/system_health.request.json"
        )
        guard
            let requestObject = try JSONSerialization.jsonObject(with: requestData)
                as? [String: String],
            let requestVersion = requestObject["client_protocol_version"]
        else {
            throw NativeHealthTestFailure.failed("health request fixture must be a string map")
        }

        let client = SystemHealthClient()
        let outgoingRequest = try JSONSerialization.data(
            withJSONObject: ["client_protocol_version": client.clientProtocolVersion],
            options: [.sortedKeys]
        )
        try require(
            outgoingRequest == requestData,
            "native client request must match the repository fixture"
        )

        let projectErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/project_errors.json"
        )
        let protocolMismatchError = try decodeFixture(
            FlitCommandError.self,
            at: "\(fixtureRoot)/protocol_mismatch.error.json"
        )
        let commandErrors = projectErrors + [protocolMismatchError]
        try require(
            Set(commandErrors.map { $0.code.rawValue }).count == 7,
            "generated command errors must decode every Project command error code"
        )
        try requireCommandError(
            try projectsListPageJson(
                afterDisplayName: nil,
                afterProjectId: nil,
                limit: 1,
                clientProtocolVersion: requestVersion
            ),
            code: .storageUnavailable,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectInspectJson(
                selectedPath: "\(root)/target/flit-macos/mismatch-must-not-be-read",
                clientProtocolVersion: "2.0"
            ),
            code: .protocolMismatch,
            fixtures: commandErrors
        )

        try initializeCore(
            dataDirectory: dataDirectory,
            clientProtocolVersion: requestVersion
        )
        guard case .ready = client.load() else {
            throw NativeHealthTestFailure.failed(
                "native client must observe validated storage"
            )
        }
        let normal = try systemHealthJson(clientProtocolVersion: requestVersion)
        let mismatch = try systemHealthJson(clientProtocolVersion: "2.0")
        let expectedNormal = try canonicalJSON(
            at: "\(fixtureRoot)/system_health.response.json"
        )
        let expectedMismatch = try canonicalJSON(
            at: "\(fixtureRoot)/protocol_mismatch.error.json"
        )
        let actualNormal = try canonicalJSON(from: normal)
        let actualMismatch = try canonicalJSON(from: mismatch)
        try require(
            actualNormal == expectedNormal,
            "normal health payload must match the repository fixture"
        )
        try require(
            actualMismatch == expectedMismatch,
            "protocol mismatch payload must match the repository fixture"
        )
        try require(coreConstructionCount() == 1, "bridge calls must share one Core construction")

        let inspectionFixture = try decodeFixture(
            FlitProjectInspectionResponse.self,
            at: "\(fixtureRoot)/project_inspect.response.json"
        )
        try require(
            inspectionFixture.protocolVersion == flitClientProtocolVersion,
            "generated inspection response must decode the current protocol fixture"
        )
        _ = try decodeFixture(
            FlitProjectInspectionRequest.self,
            at: "\(fixtureRoot)/project_inspect.request.json"
        )
        _ = try decodeFixture(
            FlitProjectRegistrationRequest.self,
            at: "\(fixtureRoot)/project_register.request.json"
        )
        for (name, expectedStatus) in [
            (
                "project_register.registered.response.json",
                FlitProjectRegistrationStatus.registered
            ),
            (
                "project_register.duplicate_canonical_path.response.json",
                FlitProjectRegistrationStatus.duplicateCanonicalPath
            ),
            (
                "project_register.duplicate_filesystem_identity.response.json",
                FlitProjectRegistrationStatus.duplicateFilesystemIdentity
            ),
        ] {
            let response = try decodeFixture(
                FlitProjectRegistrationResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.status == expectedStatus,
                "generated registration response must decode \(expectedStatus)"
            )
        }
        _ = try decodeFixture(
            FlitProjectTrustRequest.self,
            at: "\(fixtureRoot)/project_trust.request.json"
        )
        for (name, expectedStatus) in [
            (
                "project_trust.trusted.response.json",
                FlitProjectTrustStatus.trusted
            ),
            (
                "project_trust.already_trusted.response.json",
                FlitProjectTrustStatus.alreadyTrusted
            ),
        ] {
            let response = try decodeFixture(
                FlitProjectTrustResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.status == expectedStatus,
                "generated trust response must decode \(expectedStatus)"
            )
        }
        _ = try decodeFixture(
            FlitProjectsListRequest.self,
            at: "\(fixtureRoot)/projects_list.request.json"
        )
        _ = try decodeFixture(
            FlitProjectsListResponse.self,
            at: "\(fixtureRoot)/projects_list.response.json"
        )
        let driftedInspection = Data(
            """
            {
              "protocol_version": "\(flitClientProtocolVersion)",
              "canonical_path": "/tmp/flit-project",
              "selected_via_symlink": false
            }
            """.utf8
        )
        try requireDecodingFailure(
            FlitProjectInspectionResponse.self,
            from: driftedInspection,
            "generated Project decoding must reject a missing required field"
        )
        let driftedRegistration = Data(
            """
            {
              "protocol_version": "\(flitClientProtocolVersion)",
              "status": "invented_status",
              "project": null,
              "existing_project_id": null
            }
            """.utf8
        )
        try requireDecodingFailure(
            FlitProjectRegistrationResponse.self,
            from: driftedRegistration,
            "generated Project decoding must reject an unknown status"
        )

        let projectDirectory = "\(root)/target/flit-macos/native-project"
        try FileManager.default.createDirectory(
            atPath: projectDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectInspectJson(
                selectedPath: "\(root)/target/flit-macos/missing-project",
                clientProtocolVersion: requestVersion
            ),
            code: .projectInspectionFailure,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectsListPageJson(
                afterDisplayName: "Project",
                afterProjectId: nil,
                limit: 50,
                clientProtocolVersion: requestVersion
            ),
            code: .invalidProjectRequest,
            fixtures: commandErrors
        )
        let inspected = try projectInspectJson(
            selectedPath: projectDirectory,
            clientProtocolVersion: requestVersion
        )
        let inspectedObject = try JSONDecoder().decode(
            FlitProjectInspectionResponse.self,
            from: Data(inspected.utf8)
        )
        try require(
            !inspectedObject.selectedViaSymlink,
            "native Project inspection must preserve direct selection"
        )
        let registered = try projectRegisterJson(
            projectId: "native-project",
            displayName: "Native Project",
            selectedPath: projectDirectory,
            createdAt: "2026-07-27T00:00:00.000Z",
            clientProtocolVersion: requestVersion
        )
        let registeredObject = try JSONDecoder().decode(
            FlitProjectRegistrationResponse.self,
            from: Data(registered.utf8)
        )
        try require(
            registeredObject.status == .registered,
            "native Project registration must return its typed status"
        )
        let conflictDirectory = "\(root)/target/flit-macos/native-project-conflict"
        try FileManager.default.createDirectory(
            atPath: conflictDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectRegisterJson(
                projectId: "native-project",
                displayName: "Conflicting Native Project",
                selectedPath: conflictDirectory,
                createdAt: "2026-07-27T00:00:00.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectConflict,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectTrustJson(
                projectId: "missing-native-project",
                selectedPath: projectDirectory,
                confirmedAt: "2026-07-27T00:00:01.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectNotFound,
            fixtures: commandErrors
        )
        let trusted = try projectTrustJson(
            projectId: "native-project",
            selectedPath: projectDirectory,
            confirmedAt: "2026-07-27T00:00:01.000Z",
            clientProtocolVersion: requestVersion
        )
        let trustedObject = try JSONDecoder().decode(
            FlitProjectTrustResponse.self,
            from: Data(trusted.utf8)
        )
        try require(
            trustedObject.status == .trusted && trustedObject.project.trusted,
            "native Project trust must decode its exact typed result"
        )
        let listed = try projectsListPageJson(
            afterDisplayName: nil,
            afterProjectId: nil,
            limit: 50,
            clientProtocolVersion: requestVersion
        )
        let listedObject = try JSONDecoder().decode(
            FlitProjectsListResponse.self,
            from: Data(listed.utf8)
        )
        try require(
            listedObject.projects.count == 1 && listedObject.nextCursor == nil,
            "native Project list must return the registered active Project"
        )
        let driftDirectory = "\(root)/target/flit-macos/native-project-drift"
        let movedDriftDirectory = "\(driftDirectory)-moved"
        try FileManager.default.createDirectory(
            atPath: driftDirectory,
            withIntermediateDirectories: true
        )
        _ = try projectRegisterJson(
            projectId: "native-project-drift",
            displayName: "Native Project Drift",
            selectedPath: driftDirectory,
            createdAt: "2026-07-27T00:00:02.000Z",
            clientProtocolVersion: requestVersion
        )
        try FileManager.default.moveItem(
            atPath: driftDirectory,
            toPath: movedDriftDirectory
        )
        try FileManager.default.createDirectory(
            atPath: driftDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectTrustJson(
                projectId: "native-project-drift",
                selectedPath: driftDirectory,
                confirmedAt: "2026-07-27T00:00:03.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectIdentityMismatch,
            fixtures: commandErrors
        )

        guard case .ready = client.load() else {
            throw NativeHealthTestFailure.failed("repeated native health must remain ready")
        }
        try require(
            SystemHealthClient(clientProtocolVersion: "2.0").load()
                == .unavailable(messageKey: "errors.protocolMismatch"),
            "native client must fail closed when its generated version is stale"
        )

        let controller = FoundationViewController(client: client)
        _ = controller.view
        try require(controller.currentState == .ready, "foundation controller must render ready")
        try require(controller.hostedLeafCount == 1, "foundation must use one hosted SwiftUI leaf")
        try require(
            controller.view.identifier?.rawValue == "flit.foundation.root",
            "foundation root must expose a stable interface identifier"
        )
        try require(
            controller.view.accessibilityIdentifier() == "flit.foundation.root",
            "foundation root must expose a stable accessibility identifier"
        )
        let layoutWindow = NSWindow(contentViewController: controller)
        layoutWindow.minSize = NSSize(width: 720, height: 560)
        try requireFoundationLayout(
            controller,
            in: layoutWindow,
            size: NSSize(width: 1_280, height: 720),
            expectedPanelWidth: 680
        )
        try requireFoundationLayout(
            controller,
            in: layoutWindow,
            size: NSSize(width: 720, height: 560),
            expectedPanelWidth: 624
        )

        let result: [String: Any] = [
            "core_construction_count": coreConstructionCount(),
            "hosted_swiftui_leaves": controller.hostedLeafCount,
            "normal_fixture": true,
            "protocol_mismatch_fixture": true,
            "state": controller.currentState.rawValue,
        ]
        let output = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
