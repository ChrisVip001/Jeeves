// Native adapter for an explicitly selected Accessibility window. No global input events.
import AppKit
import ApplicationServices
import ScreenCaptureKit

typealias Object = [String: Any]
enum Failure: Error {
  case denied(String)
  case reselect(String)
  case uncertain(String)
  var reply: Object {
    switch self {
    case .denied(let m): return ["kind": "denied", "message": m]
    case .reselect(let m): return ["kind": "reselect", "message": m]
    case .uncertain(let m): return ["kind": "unknown", "message": m]
    }
  }
}
func attribute(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
  var value: CFTypeRef?
  guard AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success else {
    return nil
  }
  return value
}
func string(_ element: AXUIElement, _ key: String) -> String {
  attribute(element, key) as? String ?? ""
}
func elementAttribute(_ element: AXUIElement, _ key: String) -> AXUIElement? {
  guard let value = attribute(element, key), CFGetTypeID(value) == AXUIElementGetTypeID() else {
    return nil
  }
  return (value as! AXUIElement)
}
func bounds(_ element: AXUIElement) -> CGRect? {
  var point = CGPoint.zero
  var size = CGSize.zero
  guard let p = attribute(element, kAXPositionAttribute), CFGetTypeID(p) == AXValueGetTypeID(),
    let s = attribute(element, kAXSizeAttribute), CFGetTypeID(s) == AXValueGetTypeID(),
    AXValueGetValue(p as! AXValue, .cgPoint, &point), AXValueGetValue(s as! AXValue, .cgSize, &size)
  else { return nil }
  return CGRect(origin: point, size: size)
}
func alert(_ title: String, _ body: String, choices: [String]) throws -> Int {
  guard !choices.isEmpty else { throw Failure.denied("No selectable windows or applications") }
  let alert = NSAlert()
  alert.messageText = title
  alert.informativeText = body
  let menu = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 340, height: 26))
  for (i, name) in choices.enumerated() {
    menu.menu?.addItem(NSMenuItem(title: name, action: nil, keyEquivalent: ""))
    menu.lastItem?.tag = i
  }
  menu.selectItem(at: 0)
  alert.accessoryView = menu
  alert.addButton(withTitle: "Select / 选择")
  alert.addButton(withTitle: "Cancel / 取消")
  NSApp.activate(ignoringOtherApps: true)
  guard alert.runModal() == .alertFirstButtonReturn else {
    throw Failure.denied("Window selection cancelled")
  }
  return menu.indexOfSelectedItem
}
struct Control {
  let element: AXUIElement
  let label: String
  let actions: [String]
  let role: String
  let subrole: String
  let enabled: Bool
  let focused: Bool
  let value: String?
  let rect: CGRect?
}
@MainActor final class Scope {
  var app: AXUIElement?
  var window: AXUIElement?
  var pid: pid_t = 0
  var observer: AXObserver?
  var invalid = false
  var observation = ""
  var controls: [Control] = []
  var captureID: CGWindowID?

  func select() throws {
    guard AXIsProcessTrusted() else {
      throw Failure.denied(
        "Allow Accessibility for the Jeeves window helper in System Settings, then select again")
    }
    let apps = NSWorkspace.shared.runningApplications.filter {
      $0.activationPolicy == .regular && !$0.isTerminated
        && $0.processIdentifier != ProcessInfo.processInfo.processIdentifier
    }.sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
    let idx = try alert(
      "Select an application / 选择应用", "Choose the application containing the intended window.",
      choices: apps.map { $0.localizedName ?? String($0.processIdentifier) })
    let target = apps[idx]
    let application = AXUIElementCreateApplication(target.processIdentifier)
    AXUIElementSetMessagingTimeout(application, 1)
    guard let focused = elementAttribute(application, kAXFocusedWindowAttribute),
      string(focused, kAXSubroleAttribute) == kAXStandardWindowSubrole
    else { throw Failure.denied("Focus a standard window in the chosen application and try again") }
    let title = string(focused, kAXTitleAttribute)
    _ = try alert(
      "Authorize one window / 授权一个窗口",
      "Only this task can use the displayed window. Switching windows in the selected application ends the selection.",
      choices: ["\(target.localizedName ?? "") — \(title)"])
    try bindConfirmedWindow(focused, in: application, pid: target.processIdentifier)
  }

  // Production reaches this point only after the operator confirms the actual AX window.
  func bindConfirmedWindow(_ focused: AXUIElement, in application: AXUIElement, pid: pid_t) throws {
    app = application
    window = focused
    self.pid = pid
    var watch: AXObserver?
    guard
      AXObserverCreate(
        pid,
        { _, _, _, ctx in
          if let ctx {
            MainActor.assumeIsolated {
              Unmanaged<Scope>.fromOpaque(ctx).takeUnretainedValue().invalid = true
            }
          }
        }, &watch) == .success, let watch
    else { throw Failure.denied("This application cannot report window changes") }
    for (node, event) in [
      (application, kAXFocusedWindowChangedNotification),
      (focused, kAXUIElementDestroyedNotification),
    ] {
      guard
        AXObserverAddNotification(
          watch, node, event as CFString, Unmanaged.passUnretained(self).toOpaque()) == .success
      else { throw Failure.denied("This application cannot track its selected window") }
    }
    observer = watch
    CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(watch), .commonModes)
    _ = try checked()
  }
  func checked() throws -> AXUIElement {
    guard !invalid, AXIsProcessTrusted(), let app, let window,
      let focused = elementAttribute(app, kAXFocusedWindowAttribute), CFEqual(window, focused),
      (attribute(window, kAXMinimizedAttribute) as? Bool) == false,
      let children = attribute(window, kAXChildrenAttribute) as? [AXUIElement],
      !children.contains(where: { string($0, kAXRoleAttribute) == kAXSheetRole })
    else {
      throw Failure.reselect("Selected window changed or became unavailable; select it again")
    }
    return window
  }
  func control(_ node: AXUIElement) -> Control? {
    guard attribute(node, kAXEnabledAttribute) as? Bool == true else { return nil }
    let role = string(node, kAXRoleAttribute)
    let sub = string(node, kAXSubroleAttribute)
    var names: CFArray?
    AXUIElementCopyActionNames(node, &names)
    let native = names as? [String] ?? []
    var actions: [String] = []
    if [kAXButtonRole, kAXCheckBoxRole, kAXRadioButtonRole].contains(role),
      native.contains(kAXPressAction)
    {
      actions.append("click")
    }
    if role == kAXScrollBarRole
      && string(node, kAXOrientationAttribute) == kAXVerticalOrientationValue
    {
      let canStepValue = scrollPosition(node) != nil
      if native.contains(kAXDecrementAction) || canStepValue { actions.append("scrollUp") }
      if native.contains(kAXIncrementAction) || canStepValue { actions.append("scrollDown") }
    }
    if role == kAXTextFieldRole && (sub.isEmpty || sub == kAXSearchFieldSubrole)
      && attribute(node, "AXProtectedContent") as? Bool != true
    {
      var writable: DarwinBoolean = false
      if AXUIElementIsAttributeSettable(node, kAXValueAttribute as CFString, &writable) == .success
        && writable.boolValue
      {
        actions.append("setText")
      }
    }
    guard !actions.isEmpty else { return nil }
    let label =
      [string(node, kAXTitleAttribute), string(node, kAXDescriptionAttribute), role].first {
        !$0.isEmpty
      } ?? role
    let value = role == kAXTextFieldRole ? string(node, kAXValueAttribute) : ""
    return Control(
      element: node,
      label: String(label.prefix(160)),
      actions: actions,
      role: role,
      subrole: sub,
      enabled: true,
      focused: (attribute(node, kAXFocusedAttribute) as? Bool) ?? false,
      value: value.isEmpty ? nil : String(value.prefix(200)),
      rect: bounds(node)
    )
  }
  // AppKit scrollers can expose only a writable thumb position, with an implicit 0...1 range.
  func scrollPosition(_ node: AXUIElement) -> (value: Double, range: ClosedRange<Double>)? {
    var writable: DarwinBoolean = false
    guard
      AXUIElementIsAttributeSettable(node, kAXValueAttribute as CFString, &writable) == .success,
      writable.boolValue, let value = attribute(node, kAXValueAttribute) as? NSNumber
    else { return nil }
    let low = (attribute(node, kAXMinValueAttribute) as? NSNumber)?.doubleValue ?? 0
    let high = (attribute(node, kAXMaxValueAttribute) as? NSNumber)?.doubleValue ?? 1
    guard (high - low).isFinite, low < high, value.doubleValue >= low, value.doubleValue <= high
    else { return nil }
    return (value.doubleValue, low...high)
  }

  func scroll(_ node: AXUIElement, down: Bool) throws -> AXError {
    var names: CFArray?
    AXUIElementCopyActionNames(node, &names)
    let name = down ? kAXIncrementAction : kAXDecrementAction
    if (names as? [String])?.contains(name) == true {
      _ = try checked()
      return AXUIElementPerformAction(node, name as CFString)
    }
    guard let (value, range) = scrollPosition(node) else {
      throw Failure.denied("Control changed; observe again")
    }
    let step = (range.upperBound - range.lowerBound) / 10
    let next = min(range.upperBound, max(range.lowerBound, value + (down ? step : -step)))
    _ = try checked()
    return AXUIElementSetAttributeValue(node, kAXValueAttribute as CFString, NSNumber(value: next))
  }
  func scan(_ root: AXUIElement) -> ([Control], Bool) {
    var result: [Control] = []
    var queue = [root]
    var offset = 0
    while offset < queue.count && offset < 512 && result.count < 128 {
      let node = queue[offset]
      offset += 1
      if string(node, kAXSubroleAttribute) == kAXSecureTextFieldSubrole { continue }
      if let item = control(node) { result.append(item) }
      var children: CFArray?
      let remaining = 512 - queue.count
      if remaining > 0,
        AXUIElementCopyAttributeValues(
          node, kAXChildrenAttribute as CFString, 0, remaining, &children) == .success
      {
        queue.append(contentsOf: children as? [AXUIElement] ?? [])
      }
    }
    return (result, result.count >= 128 || offset < queue.count)
  }
  func observe() throws -> Object {
    let root = try checked()
    let (found, truncated) = scan(root)
    controls = found
    _ = try checked()
    observation = UUID().uuidString
    return [
      "kind": "observed",
      "observation": [
        "observationId": observation, "window": String(string(root, kAXTitleAttribute).prefix(256)),
        "truncated": truncated,
        "elements": controls.enumerated().map { index, control in
          var rect: Object? = nil
          if let r = control.rect {
            rect = [
              "x": Int(r.origin.x), "y": Int(r.origin.y),
              "width": Int(r.size.width), "height": Int(r.size.height),
            ]
          }
          return [
            "id": index, "label": control.label, "actions": control.actions,
            "role": control.role, "subrole": control.subrole,
            "enabled": control.enabled, "focused": control.focused,
            "value": control.value as Any, "bounds": rect as Any,
          ] as Object
        },
      ] as Object,
    ]
  }
  func act(_ command: Object) throws -> Object {
    let root = try checked()
    guard command["observationId"] as? String == observation, let id = command["elementId"] as? Int,
      controls.indices.contains(id),
      let action = command["action"] as? String
    else { throw Failure.denied("Observe the window again") }
    let ref = controls[id]
    observation = ""
    guard ref.actions.contains(action),
      scan(root).0.contains(where: {
        CFEqual($0.element, ref.element) && $0.label == ref.label && $0.actions.contains(action)
      }),
      let owner = elementAttribute(ref.element, kAXTopLevelUIElementAttribute), CFEqual(owner, root)
    else { throw Failure.denied("Control changed; observe again") }
    _ = try checked()
    let code: AXError
    switch action {
    case "click": code = AXUIElementPerformAction(ref.element, kAXPressAction as CFString)
    case "scrollUp", "scrollDown": code = try scroll(ref.element, down: action == "scrollDown")
    case "setText":
      guard let text = command["text"] as? String, text.utf8.count <= 2048,
        text.rangeOfCharacter(from: .controlCharacters) == nil
      else { throw Failure.denied("Expected a single plain-text field value") }
      code = AXUIElementSetAttributeValue(
        ref.element, kAXValueAttribute as CFString, text as CFString)
    default: throw Failure.denied("Unsupported action")
    }
    guard code == .success else {
      throw Failure.uncertain("Application did not confirm the action; inspect the window")
    }
    do { return try observe() } catch {
      throw Failure.uncertain("Window changed after dispatch; inspect the result")
    }
  }
  // Stage Manager can temporarily replace an inactive app's visible windows with
  // thumbnails. Wait for the confirmed AX window and exact on-screen identity.
  @available(macOS 14.0, *)
  func captureTarget(_ root: AXUIElement) async throws -> SCWindow {
    guard let application = NSRunningApplication(processIdentifier: pid), let app else {
      throw Failure.reselect("Selected window changed or became unavailable; select it again")
    }
    var requestedActivation = false
    let deadline = ContinuousClock.now + .seconds(3)
    repeat {
      _ = try checked()
      let content = try await SCShareableContent.excludingDesktopWindows(
        true, onScreenWindowsOnly: true)
      _ = try checked()
      let title = string(root, kAXTitleAttribute)
      if !title.isEmpty, let rect = bounds(root),
        let siblings = attribute(app, kAXWindowsAttribute) as? [AXUIElement]
      {
        let peers = siblings.filter { string($0, kAXTitleAttribute) == title && bounds($0) == rect }
        let matches = content.windows.filter {
          $0.owningApplication?.processID == pid && $0.windowLayer == 0 && $0.title == title
            && $0.frame == rect
        }
        guard peers.count <= 1, matches.count <= 1 else {
          throw Failure.denied("Cannot uniquely identify this window for capture")
        }
        if let peer = peers.first, let target = matches.first {
          guard CFEqual(peer, root), captureID == nil || captureID == target.windowID else {
            throw Failure.reselect("Window identity changed; select it again")
          }
          return target
        }
      }
      if !application.isActive && !requestedActivation {
        NSApp.yieldActivation(to: application)
        guard application.activate(options: []) else {
          throw Failure.denied("Bring the selected window to the front and preview again")
        }
        requestedActivation = true
      }
      try await Task.sleep(for: .milliseconds(100))
    } while ContinuousClock.now < deadline
    throw Failure.denied("Cannot uniquely identify this window for capture")
  }
  @available(macOS 14.0, *)
  func frame(_ command: Object) async throws -> Object {
    let root = try checked()
    guard command["observationId"] as? String == observation else {
      throw Failure.denied("Preview requires a fresh observation")
    }
    guard CGPreflightScreenCaptureAccess() else {
      throw Failure.denied(
        "Allow Screen Recording for the Jeeves window helper in System Settings, then preview again"
      )
    }
    let target = try await captureTarget(root)
    _ = try checked()
    captureID = target.windowID
    let rect = target.frame
    let config = SCStreamConfiguration()
    let scale = min(1, 1000 / max(1, max(rect.width, rect.height)))
    config.width = max(1, Int(rect.width * scale))
    config.height = max(1, Int(rect.height * scale))
    config.showsCursor = false
    config.ignoreShadowsSingleWindow = true
    let image = try await SCScreenshotManager.captureImage(
      contentFilter: SCContentFilter(desktopIndependentWindow: target), configuration: config)
    _ = try checked()
    guard
      let jpeg = NSBitmapImageRep(cgImage: image).representation(
        using: .jpeg, properties: [.compressionFactor: 0.7]), jpeg.count <= 800_000
    else { throw Failure.denied("Window preview exceeds the size limit") }
    return ["kind": "frame", "jpeg": jpeg.base64EncodedString()]
  }
}
