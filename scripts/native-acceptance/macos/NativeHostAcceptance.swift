import XCTest
import AppKit
import CryptoKit

final class NativeHostAcceptance: XCTestCase {
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    var steps: [[String: Any]] = []
    var phase = "before-launch"
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }

    override func setUpWithError() throws { continueAfterFailure = false }

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

    func assertFounderBadge() throws {
        try click("Identity", timeout: 210)
        XCTAssertTrue(annex.staticTexts["Server founder"].firstMatch.waitForExistence(timeout: 30),
                      "The actual profile must identify the host as Server founder")
        capture("host-founder-profile-visible")
        annex.typeKey(.escape, modifierFlags: [])
    }

    func publicSummary(_ origin: String) throws -> [String: Any] {
        let complete = expectation(description: "Real HTTPS public server summary")
        var responseData: Data?
        var responseStatus: Int?
        var responseError: Error?
        let url = try XCTUnwrap(URL(string: origin + "/api/public/server/summary"))
        var request = URLRequest(url: url)
        request.timeoutInterval = 25
        URLSession.shared.dataTask(with: request) { data, response, error in
            responseData = data
            responseStatus = (response as? HTTPURLResponse)?.statusCode
            responseError = error
            complete.fulfill()
        }.resume()
        wait(for: [complete], timeout: 30)
        XCTAssertNil(responseError)
        XCTAssertEqual(responseStatus, 200)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(responseData)) as? [String: Any])
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
        try assertFounderBadge()
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

        var encoded = String(url.path.dropFirst("/invite/".count)).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        let payload = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(Data(base64Encoded: encoded))) as? [String: Any])
        let destination = try XCTUnwrap(payload["server"] as? String)
        let publicURL = try XCTUnwrap(URL(string: destination))
        XCTAssertEqual(publicURL.scheme, "https")
        XCTAssertEqual(publicURL.host, "www.monolithannex.com")
        XCTAssertNotNil(publicURL.path.range(of: "^/host/[a-f0-9]{64}$", options: .regularExpression))
        let before = try XCTUnwrap(databaseSnapshot())
        let servers = try XCTUnwrap(before["servers"] as? [[String: Any]])
        XCTAssertEqual(servers.count, 1)
        let server = try XCTUnwrap(servers.first)
        let slug = try XCTUnwrap(server["slug"] as? String)
        XCTAssertEqual(server["public_url"] as? String, destination)
        let summary = try publicSummary(destination)
        XCTAssertEqual(summary["slug"] as? String, slug,
                       "The public invite must reach the same server the EXE/app provisioned")
        XCTAssertEqual(summary["label"] as? String, server["label"] as? String)
        XCTAssertEqual(summary["public_url"] as? String, destination)
        capture("host-public-destination-verified")

        let identitiesBefore = try XCTUnwrap(before["identities"] as? [[String: Any]])
        let founderID = try XCTUnwrap(identitiesBefore.first?["id"] as? Int)
        annex.terminate()
        annex.launch()
        try assertFounderBadge()
        let retained = NSPredicate { _, _ in
            guard let snapshot = self.databaseSnapshot(),
                  let identities = snapshot["identities"] as? [[String: Any]],
                  let servers = snapshot["servers"] as? [[String: Any]],
                  identities.count == 1, servers.count == 1 else { return false }
            return (identities[0]["id"] as? Int) == founderID &&
                   (identities[0]["can_moderate"] as? Int) == 1 &&
                   (identities[0]["can_invite"] as? Int) == 1 &&
                   (servers[0]["slug"] as? String) == slug &&
                   (servers[0]["public_url"] as? String) == destination
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: retained, object: nil)], timeout: 15), .completed)
        XCTAssertEqual(try publicSummary(destination)["slug"] as? String, slug)
        capture("host-founder-and-public-address-retained-after-reopen")
        let receipt: [String: Any] = ["passed": true, "host_clicks": 1, "separate_identity_click": prerequisiteIdentityClick,
            "invite_sha256": SHA256.hash(data: Data(invite.utf8)).map { String(format: "%02x", $0) }.joined(),
            "routing_origin": url.host!, "destination": destination, "server_slug": slug,
            "founder_badge_visible": true, "founder_retained_after_reopen": true,
            "public_destination_verified": true, "joined_recipient_proven": false]
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted]), uniformTypeIdentifier: "public.json")
        attachment.name = "annex-native-host-result"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
