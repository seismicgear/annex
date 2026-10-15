import XCTest
import AppKit
import CryptoKit

final class NativeHostAcceptance: XCTestCase {
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    var steps: [[String: Any]] = []
    var phase = "before-launch"
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }

    func capture(_ name: String) {
        phase = name
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name; shot.lifetime = .keepAlways; add(shot)
        let tree = XCTAttachment(string: annex.debugDescription)
        tree.name = name + "-accessibility"; tree.lifetime = .keepAlways; add(tree)
        steps.append(["phase": name, "at": ISO8601DateFormatter().string(from: Date())])
    }

    override func tearDownWithError() throws {
        capture("host-final-" + phase)
        let result = XCTAttachment(data: try JSONSerialization.data(withJSONObject: steps, options: [.prettyPrinted]), uniformTypeIdentifier: "public.json")
        result.name = "annex-native-host-events"; result.lifetime = .keepAlways; add(result)
        annex.terminate()
    }

    func click(_ title: String, timeout: TimeInterval = 30) throws {
        let button = annex.buttons[title].firstMatch
        guard button.waitForExistence(timeout: timeout) else {
            capture("host-missing-" + title)
            XCTFail("Published app did not expose " + title + " at " + phase)
            throw NSError(domain: "AnnexHostAcceptance", code: 1)
        }
        button.click()
    }

    func databaseSnapshot() -> [String: Any]? {
        guard let raw = try? String(contentsOf: evidence.appendingPathComponent("host-observations.jsonl"), encoding: .utf8),
              let last = raw.split(separator: "\n").last,
              let data = String(last).data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    func testPublishedHostSetup() throws {
        // Launch the installed, unmodified release with its ordinary defaults.
        // No API-created founder, injected token, configured public URL or proxy.
        annex.launch()
        XCTAssertTrue(annex.windows.firstMatch.waitForExistence(timeout: 40))
        capture("host-fresh-install")
        var prerequisiteIdentityClick = false
        if annex.buttons["Create New Identity"].firstMatch.waitForExistence(timeout: 5) {
            prerequisiteIdentityClick = true
            try click("Create New Identity")
        }
        try click("Start Hosting", timeout: 60)
        capture("host-clicked-once")
        try click("Admin", timeout: 210)
        capture("host-admin-available")
        let founder = NSPredicate { _, _ in
            guard let rows = self.databaseSnapshot()?["identities"] as? [[String: Any]] else { return false }
            return rows.count == 1 && (rows[0]["can_moderate"] as? Int) == 1 && (rows[0]["can_invite"] as? Int) == 1
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: founder, object: nil)], timeout: 15), .completed)
        try click("Server Settings")
        capture("host-settings-without-manual-configuration")
        try click("Copy Link", timeout: 45)
        let invite = try XCTUnwrap(NSPasteboard.general.string(forType: .string))
        let url = try XCTUnwrap(URL(string: invite))
        XCTAssertEqual(url.scheme, "https")
        XCTAssertTrue(["monolithannex.com", "www.monolithannex.com"].contains(url.host ?? ""))
        XCTAssertTrue(url.path.hasPrefix("/invite/"))
        XCTAssertFalse(prerequisiteIdentityClick, "Host must provision its identity without a separate identity-creation click")
        let receipt: [String: Any] = ["host_clicks": 1, "separate_identity_click": prerequisiteIdentityClick,
            "invite_sha256": SHA256.hash(data: Data(invite.utf8)).map { String(format: "%02x", $0) }.joined(),
            "routing_origin": url.host!, "joined_recipient_proven": false]
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted]), uniformTypeIdentifier: "public.json")
        attachment.name = "annex-native-host-result"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
