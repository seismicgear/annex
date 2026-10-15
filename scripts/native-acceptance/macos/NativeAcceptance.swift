import XCTest
import AppKit

final class NativeAcceptance: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

    func capture(_ app: XCUIApplication, _ name: String) {
        print("ANNEX_UI_HIERARCHY \(name)\n\(app.debugDescription)")
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = name
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let hierarchy = XCTAttachment(string: app.debugDescription)
        hierarchy.name = name + "-accessibility"
        hierarchy.lifetime = .keepAlways
        add(hierarchy)
    }

    func testNativeDesktopAvailability() throws {
        let safari = XCUIApplication(bundleIdentifier: "com.apple.Safari")
        defer { capture(safari, "actual-macos-public-annex-page") }
        safari.launch()
        XCTAssertTrue(safari.windows.firstMatch.waitForExistence(timeout: 30))
        safari.typeKey("l", modifierFlags: .command)
        safari.typeText("https://www.monolithannex.com/")
        safari.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(safari.webViews.firstMatch.waitForExistence(timeout: 60))
        let page = safari.webViews.firstMatch
        XCTAssertTrue(page.staticTexts.containing(NSPredicate(format: "label CONTAINS[c] %@", "Annex")).firstMatch.waitForExistence(timeout: 60))
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "actual-macos-public-annex-page"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let hierarchy = XCTAttachment(string: safari.debugDescription)
        hierarchy.name = "actual-safari-accessibility"
        hierarchy.lifetime = .keepAlways
        add(hierarchy)
        print("ANNEX_MACOS_NATIVE_UI_AVAILABLE")
    }

    func testPublishedApplicationStarts() throws {
        let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
        defer { capture(annex, "actual-published-annex-startup") }
        annex.launch()
        XCTAssertTrue(annex.windows.firstMatch.waitForExistence(timeout: 45))
        XCTAssertTrue(annex.buttons["Create New Identity"].waitForExistence(timeout: 60))
        print("ANNEX_PUBLISHED_MACOS_STARTUP_PASSED")
    }
}
