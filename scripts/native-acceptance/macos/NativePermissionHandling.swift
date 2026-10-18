import XCTest
import AppKit

extension XCTestCase {
    @discardableResult
    private func acceptAnnexLocalNetworkPrompt(_ element: XCUIElement) -> Bool {
        let description = element.debugDescription
        guard description.contains("Annex"),
              description.contains("local network") else { return false }
        let allow = element.descendants(matching: .button).matching(
            NSPredicate(format: "label == %@ OR title == %@", "Allow", "Allow")).firstMatch
        guard allow.exists, allow.isHittable else { return false }
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "annex-local-network-permission"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let tree = XCTAttachment(string: description)
        tree.name = "annex-local-network-permission-accessibility"
        tree.lifetime = .keepAlways
        add(tree)
        allow.click()
        return true
    }

    func acceptVisibleAnnexLocalNetworkPermission() {
        // The macOS 15 prompt can belong to a system application rather than
        // either test application, so XCTest's interruption monitor may not fire.
        // Discover its real running owner; never launch or modify system apps.
        for running in NSWorkspace.shared.runningApplications {
            guard let bundle = running.bundleIdentifier,
                  bundle.hasPrefix("com.apple."),
                  ["notification", "controlcenter", "securityagent"].contains(where: {
                      bundle.lowercased().contains($0)
                  }) else { continue }
            let owner = XCUIApplication(bundleIdentifier: bundle)
            for window in owner.windows.allElementsBoundByIndex {
                if acceptAnnexLocalNetworkPrompt(window) { return }
            }
        }
    }

    func monitorAnnexLocalNetworkPermission() {
        // macOS 15 can show this ordinary first-launch consent after Host has
        // already opened. Handle only Annex's local-network request on these
        // disposable acceptance machines; never dismiss unrelated dialogs.
        addUIInterruptionMonitor(withDescription: "Annex local-network permission") { alert in
            self.acceptAnnexLocalNetworkPrompt(alert)
        }
    }
}
