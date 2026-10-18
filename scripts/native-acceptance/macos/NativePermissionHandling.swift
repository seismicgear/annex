import XCTest

extension XCTestCase {
    func monitorAnnexLocalNetworkPermission() {
        // macOS 15 can show this ordinary first-launch consent after Host has
        // already opened. Handle only Annex's local-network request on these
        // disposable acceptance machines; never dismiss unrelated dialogs.
        addUIInterruptionMonitor(withDescription: "Annex local-network permission") { alert in
            let description = alert.debugDescription
            guard description.contains("Annex"),
                  description.contains("local network") else { return false }
            let allow = alert.descendants(matching: .any).matching(
                NSPredicate(format: "label == %@ OR title == %@", "Allow", "Allow")).firstMatch
            guard allow.exists, allow.isHittable else { return false }
            let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
            screenshot.name = "annex-local-network-permission"
            screenshot.lifetime = .keepAlways
            self.add(screenshot)
            let tree = XCTAttachment(string: description)
            tree.name = "annex-local-network-permission-accessibility"
            tree.lifetime = .keepAlways
            self.add(tree)
            allow.click()
            return true
        }
    }
}
