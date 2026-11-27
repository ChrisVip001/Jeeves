// Exercises the same native scope used after confirmation, only against the two windows
// of the fixture process this check launches. No installed helper or user home is changed.
import AppKit

struct CheckError: LocalizedError {
  let errorDescription: String?
  init(_ text: String) { errorDescription = text }
}

@main @MainActor struct NativeWindowCheck {
  static func require(_ condition: Bool, _ reason: String) throws {
    if !condition { throw CheckError(reason) }
  }

  static func waitFor(_ reason: String, _ condition: () -> Bool) async throws {
    for _ in 0..<60 {
      if condition() { return }
      try await Task.sleep(for: .milliseconds(50))
    }
    throw CheckError(reason)
  }

  static func act(_ action: String, label: String? = nil, scope: Scope) throws {
    _ = try scope.observe()
    guard
      let index = scope.controls.firstIndex(where: {
        $0.actions.contains(action) && (label == nil || label == $0.label)
      })
    else { throw CheckError("Missing fixture action: " + action) }
    _ = try scope.act(["observationId": scope.observation, "elementId": index, "action": action])
  }

  static func verifyFrame(_ scope: Scope, title: String) async throws {
    _ = try scope.observe()
    let result = try await scope.frame(["observationId": scope.observation])
    guard let base64 = result["jpeg"] as? String, let data = Data(base64Encoded: base64),
      let bitmap = NSBitmapImageRep(data: data)
    else { throw CheckError("No JPEG preview") }
    var pink = 0
    var cyan = 0
    for y in stride(from: 0, to: bitmap.pixelsHigh, by: 2) {
      for x in stride(from: 0, to: bitmap.pixelsWide, by: 2) {
        guard let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
        let (r, g, b) = (c.redComponent, c.greenComponent, c.blueComponent)
        if r - g > 0.3 && b - g > 0.3 { pink += 1 }
        if g - r > 0.3 && b - r > 0.3 { cyan += 1 }
      }
    }
    try require(
      title == "Jeeves Fixture 1" ? pink > 100 && cyan < 10 : cyan > 100 && pink < 10,
      "Wrong window marker in JPEG: \(title), pink=\(pink), cyan=\(cyan)")
    print("PASS isolated preview of \(title)")
  }

  static func verifyVisualWindow(_ app: AXUIElement, pid: pid_t) async throws {
    let driver = try CuaDriver()
    guard let window = try driver.windows(pid: pid).first(where: { $0.title == "Jeeves Fixture 2" }),
      let focused = elementAttribute(app, kAXFocusedWindowAttribute)
    else { throw CheckError("Fixture window missing") }
    try require(
      try driver.windowEntries(pid: pid).contains {
        $0["window_id"] as? UInt32 == window.id && ($0["layer"] as? Int ?? 0) != 0
      },
      "Visual fixture did not exercise the foreground route")
    let scope = Scope()
    try scope.bindConfirmedWindow(focused, in: app, pid: pid)
    func observeCheckbox() throws -> Object {
      guard let read = try window.observe()["observation"] as? Object,
        let source = read["observationId"] as? String,
        let elements = read["elements"] as? [Object],
        let checkbox = elements.first(where: { $0["label"] as? String == "Sample checkbox" }),
        let id = checkbox["id"] as? Int
      else { throw CheckError("The selected window did not expose its visible checkbox") }
      return ["observationId": source, "elementId": id, "action": "click"]
    }
    let stale = try observeCheckbox()
    try act("click", label: "Sample checkbox", scope: scope)
    do {
      _ = try window.act(stale)
      throw CheckError("An action grounded in a changed image was accepted")
    } catch Failure.denied("Window image changed; observe again") {}
    let action = try observeCheckbox()
    let preview = try window.frame(action)
    try require(preview["kind"] as? String == "frame", "Missing visual preview")
    let reply = try window.act(action)
    try require(reply["kind"] as? String == "observed", "Click did not produce an updated observation")
    _ = try scope.observe()
    let selected = scope.controls.first(where: { $0.label == "Sample checkbox" })
    try require(
      selected.flatMap { attribute($0.element, kAXValueAttribute) as? NSNumber }?.intValue == 0,
      "The fixture did not independently confirm the checkbox click")
    do {
      _ = try window.act(action)
      throw CheckError("Replayed visual action was accepted")
    } catch Failure.denied {}
    print("PASS Cua Driver foreground window preview, click readback, changed image and stale reference rejection")
  }

  static func run() async throws {
    guard AXIsProcessTrusted(), CGPreflightScreenCaptureAccess() else {
      throw CheckError(
        "Native window check requires operator-granted Accessibility and Screen Recording")
    }
    let fixture = Process()
    fixture.executableURL = URL(fileURLWithPath: CommandLine.arguments[1])
    if CommandLine.arguments.contains("--cua") { fixture.arguments = ["--visual"] }
    try fixture.run()
    defer {
      fixture.terminate()
      fixture.waitUntilExit()
    }
    let app = AXUIElementCreateApplication(fixture.processIdentifier)
    var first: AXUIElement?
    try await waitFor("Fixture 2 did not open") {
      first = elementAttribute(app, kAXFocusedWindowAttribute)
      return first.map { string($0, kAXTitleAttribute) } == "Jeeves Fixture 2"
    }
    guard let first else { throw CheckError("Fixture 2 missing") }
    if CommandLine.arguments.contains("--cua") {
      try await Task.sleep(for: .seconds(1))
      try await verifyVisualWindow(app, pid: fixture.processIdentifier)
      return
    }
    let scope = Scope()
    try scope.bindConfirmedWindow(first, in: app, pid: fixture.processIdentifier)
    // A separate app takes focus as the cockpit would; the helper itself stays in the background.
    let cockpit = Process()
    cockpit.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
    cockpit.arguments = ["--cockpit"]
    try cockpit.run()
    defer {
      cockpit.terminate()
      cockpit.waitUntilExit()
    }
    try await waitFor("Fixture cockpit did not take focus") {
      NSRunningApplication(processIdentifier: cockpit.processIdentifier)?.isActive == true
    }
    // Allow the Stage Manager transition to finish on Macs where it is enabled.
    try await Task.sleep(for: .seconds(1))
    try await verifyFrame(scope, title: "Jeeves Fixture 2")
    guard
      let area = (attribute(first, kAXChildrenAttribute) as? [AXUIElement])?.first(where: {
        string($0, kAXRoleAttribute) == kAXScrollAreaRole
      }),
      let bar = elementAttribute(area, kAXVerticalScrollBarAttribute)
    else { throw CheckError("Fixture scroll area missing") }
    func position() -> Double? { (attribute(bar, kAXValueAttribute) as? NSNumber)?.doubleValue }
    guard let initial = position() else { throw CheckError("Scroll position unavailable") }
    guard
      let text = (attribute(area, kAXChildrenAttribute) as? [AXUIElement])?.first(where: {
        string($0, kAXRoleAttribute) == kAXTextAreaRole
      })
    else { throw CheckError("Fixture text area missing") }
    func firstVisibleCharacter() -> Int? {
      guard let raw = attribute(text, kAXVisibleCharacterRangeAttribute),
        CFGetTypeID(raw) == AXValueGetTypeID()
      else { return nil }
      var visible = CFRange()
      return AXValueGetValue(raw as! AXValue, .cfRange, &visible) ? visible.location : nil
    }
    guard let top = firstVisibleCharacter() else {
      throw CheckError("Visible text range unavailable")
    }
    try act("scrollDown", scope: scope)
    try await waitFor("List did not move down") {
      position().map { $0 > initial } == true && firstVisibleCharacter().map { $0 > top } == true
    }
    try act("scrollUp", scope: scope)
    try await waitFor("List did not return to top") {
      position() == initial && firstVisibleCharacter() == top
    }
    print("PASS scrollbar and visible text moved down and returned to the top")
    _ = try scope.observe()
    let old = scope.observation
    guard let button = scope.controls.firstIndex(where: { $0.label == "Switch fixture window" })
    else {
      throw CheckError("Switch button missing")
    }
    let click: Object = ["observationId": old, "elementId": button, "action": "click"]
    do { _ = try scope.act(click) } catch Failure.uncertain {}
    try await waitFor("Fixture did not change focus") {
      elementAttribute(app, kAXFocusedWindowAttribute).map { string($0, kAXTitleAttribute) }
        == "Jeeves Fixture 1"
    }
    do {
      _ = try scope.observe()
      throw CheckError("Switched window still observable")
    } catch Failure.reselect {}
    do {
      _ = try await scope.frame(["observationId": old])
      throw CheckError("Stale window captured")
    } catch Failure.reselect {}
    do {
      _ = try scope.act(click)
      throw CheckError("Stale action accepted")
    } catch Failure.reselect {}
    print("PASS switch revoked old observation, action and preview")
    let next = Scope()
    guard let second = elementAttribute(app, kAXFocusedWindowAttribute) else {
      throw CheckError("Fixture 1 missing")
    }
    try next.bindConfirmedWindow(second, in: app, pid: fixture.processIdentifier)
    try await verifyFrame(next, title: "Jeeves Fixture 1")
    try act("click", label: "Match fixture windows", scope: next)
    do {
      _ = try await next.frame(["observationId": next.observation])
      throw CheckError("Ambiguous identity produced a preview")
    } catch Failure.denied("Cannot uniquely identify this window for capture") {}
    print("PASS ambiguous title and bounds refused without a preview")
  }

  static func main() {
    let app = NSApplication.shared
    if CommandLine.arguments.dropFirst().first == "--cockpit" {
      app.setActivationPolicy(.regular)
      let window = NSWindow(
        contentRect: NSRect(x: 40, y: 50, width: 180, height: 100), styleMask: [.titled],
        backing: .buffered, defer: false)
      window.title = "Jeeves local acceptance"
      window.makeKeyAndOrderFront(nil)
      app.activate(ignoringOtherApps: true)
      withExtendedLifetime(window) { app.run() }
      return
    }
    app.setActivationPolicy(.accessory)
    Task { @MainActor in
      do {
        try await run()
        exit(0)
      } catch let failure as Failure {
        print("FAIL:", failure.reply)
        exit(1)
      } catch {
        print("FAIL: \(error.localizedDescription)")
        exit(1)
      }
    }
    app.run()
  }
}
