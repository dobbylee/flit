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
        let fixtureRoot = "\(root)/fixtures/protocol/commands/v1.0"

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

        guard case .ready = client.load() else {
            throw NativeHealthTestFailure.failed("native client must accept matching health")
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
