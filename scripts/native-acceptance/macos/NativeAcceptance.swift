import XCTest
import AppKit

final class NativeAcceptance: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

    func testNativeDesktopAvailability() throws {
        let safari = XCUIApplication(bundleIdentifier: "com.apple.Safari")
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
}
