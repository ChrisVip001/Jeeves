// Exercise only the web content of the fixture process on its reserved loopback origin.
import AppKit
import ApplicationServices

struct ProbeError: LocalizedError {
  let message: String
  var errorDescription: String? { message }
  init(_ message: String) { self.message = message }
}

func tree(_ root: AXUIElement) -> [AXUIElement] {
  var stack = [root]
  var result = [AXUIElement]()
  while let node = stack.popLast() {
    result.append(node)
    if let children = attribute(node, kAXChildrenAttribute) as? [AXUIElement] {
      stack.append(contentsOf: children.reversed())
    }
  }
  return result
}

func titleOf(_ node: AXUIElement) -> String {
  for key in [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute] {
    let value = string(node, key)
    if !value.isEmpty { return value }
  }
  return ""
}

@main struct CockpitWindow {
  static func run(_ args: [String]) throws -> Any {
    guard args.count >= 4, let pid = Int32(args[0]),
      let app = NSRunningApplication(processIdentifier: pid),
      app.bundleIdentifier == "app.jeeves.desktop",
      app.bundleURL?.resolvingSymlinksInPath().path
        == URL(fileURLWithPath: args[1]).resolvingSymlinksInPath().path
    else { throw ProbeError("Fixture application unavailable") }
    if args[3] == "quit" { return app.terminate() }
    guard AXIsProcessTrusted() else { throw ProbeError("Accessibility unavailable") }
    let ax = AXUIElementCreateApplication(pid)
    if args[3] == "activate" {
      guard app.activate(options: []) else { throw ProbeError("Cannot activate the fixture") }
      return true
    }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
      throw ProbeError("The fixture is not the foreground application")
    }
    guard let window = elementAttribute(ax, kAXFocusedWindowAttribute) else {
      throw ProbeError("Fixture has no focused native window")
    }
    let web = tree(window).first { node in
      guard string(node, kAXRoleAttribute) == "AXWebArea",
        let url = attribute(node, kAXURLAttribute) as? URL
      else { return false }
      return url.scheme == "http" && url.host == "127.0.0.1" && url.port.map(String.init) == args[2]
    }
    guard let web else {
      throw ProbeError("No web area for the isolated Runtime in the focused window")
    }
    let nodes = tree(web)
    if args[3] == "inspect" {
      return nodes.map {
        [
          "role": string($0, kAXRoleAttribute), "id": string($0, "AXDOMIdentifier"),
          "label": titleOf($0), "value": string($0, kAXValueAttribute),
        ]
      }
    }
    if args[3] == "window" {
      let rows =
        CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
      let matches = rows.filter {
        ($0[kCGWindowOwnerPID as String] as? Int32) == pid
          && ($0[kCGWindowLayer as String] as? Int) == 0
          && ($0[kCGWindowName as String] as? String) == string(window, kAXTitleAttribute)
      }
      guard matches.count == 1, let id = matches[0][kCGWindowNumber as String] else {
        throw ProbeError("Cannot identify a single visible fixture window")
      }
      return [
        "pid": pid, "window": id, "title": string(window, kAXTitleAttribute), "frontmost": true,
        "screenCapture": CGPreflightScreenCaptureAccess(),
      ]
    }
    guard args.count >= 5 else { throw ProbeError("Missing fixture control") }
    let controls = nodes.filter { node in
      string(node, "AXDOMIdentifier") == args[4]
        || (string(node, kAXRoleAttribute) == kAXButtonRole && titleOf(node) == args[4])
    }
    guard controls.count == 1, let control = controls.first else {
      throw ProbeError("Expected one fixture control for \(args[4]), got \(controls.count)")
    }
    let result: AXError
    switch args[3] {
    case "press": result = AXUIElementPerformAction(control, kAXPressAction as CFString)
    case "fill" where args.count == 6:
      guard string(control, kAXRoleAttribute) == kAXTextAreaRole,
        AXUIElementSetAttributeValue(control, kAXFocusedAttribute as CFString, kCFBooleanTrue)
          == .success,
        let focused = elementAttribute(ax, kAXFocusedUIElementAttribute), CFEqual(focused, control),
        string(control, kAXValueAttribute).isEmpty
      else {
        throw ProbeError("Fixture composer must be empty and focused")
      }
      // WebKit dispatches native input events for typed text. Setting AXValue alone
      // can change its display without notifying a reactive form.
      var expected = ""
      for character in args[5] {
        let units = Array(String(character).utf16)
        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false)
        else { throw ProbeError("Unable to create fixture keystroke") }
        down.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        up.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        down.postToPid(pid)
        usleep(12000)
        up.postToPid(pid)
        expected.append(character)
        let deadline = Date().addingTimeInterval(1.5)
        while string(control, kAXValueAttribute) != expected && Date() < deadline {
          usleep(10000)
        }
        guard string(control, kAXValueAttribute) == expected else {
          throw ProbeError(
            "WebKit did not accept the fixture keystroke at position \(expected.count)")
        }
      }
      result = .success
    default: throw ProbeError("Unknown fixture action")
    }
    guard result == .success else { throw ProbeError("Native control returned \(result.rawValue)") }
    return true
  }

  static func main() {
    do {
      let result = try run(Array(CommandLine.arguments.dropFirst()))
      let data = try JSONSerialization.data(withJSONObject: result, options: [.fragmentsAllowed])
      FileHandle.standardOutput.write(data)
    } catch {
      FileHandle.standardError.write(Data((error.localizedDescription + "\n").utf8))
      exit(1)
    }
  }
}
