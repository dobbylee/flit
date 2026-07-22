import SwiftUI

enum FoundationState: String, Sendable {
    case checking
    case ready
    case unavailable

    var statusCopy: FoundationCopyKey {
        switch self {
        case .checking: .statusChecking
        case .ready: .statusReady
        case .unavailable: .statusUnavailable
        }
    }

    var boundaryCopy: FoundationCopyKey {
        switch self {
        case .checking: .boundaryChecking
        case .ready: .boundaryReady
        case .unavailable: .boundaryUnavailable
        }
    }

    var color: Color {
        switch self {
        case .checking: Color(red: 0.60, green: 0.45, blue: 0.19)
        case .ready: Color(red: 0.19, green: 0.54, blue: 0.38)
        case .unavailable: Color(red: 0.64, green: 0.29, blue: 0.29)
        }
    }
}

struct FoundationStatusBadge: View {
    let state: FoundationState

    var body: some View {
        HStack(spacing: 12) {
            Circle()
                .fill(state.color)
                .frame(width: 10, height: 10)
                .accessibilityHidden(true)

            Text(FoundationCopy.text(state.statusCopy))
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(state == .unavailable ? Color.red : Color.primary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("flit.foundation.status")
    }
}
