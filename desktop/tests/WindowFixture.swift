// Manual native acceptance target. Both windows contain only disposable local controls.
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.regular)
final class Switcher: NSObject {
  var windows: [NSWindow] = []
  @MainActor @objc func switchWindow(_ button: NSButton) {
    windows.first { $0 !== button.window }?.makeKeyAndOrderFront(nil)
  }
  @MainActor @objc func matchWindows(_ button: NSButton) {
    guard let selected = button.window, let other = windows.first(where: { $0 !== selected }) else {
      return
    }
    other.title = selected.title
    other.setFrame(selected.frame, display: true)
  }
}
let switcher = Switcher()
for index in 1...2 {
  let window = NSWindow(
    contentRect: NSRect(x: 160 + index * 60, y: 180 + index * 50, width: 360, height: 340),
    styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
  window.title = "Jeeves Fixture \(index)"
  window.isReleasedWhenClosed = false
  let field = NSTextField(string: "Sample text")
  field.setAccessibilityLabel("Sample field")
  let secure = NSSecureTextField(string: "")
  secure.placeholderString = "Secure test field"
  let toggle = NSButton(checkboxWithTitle: "Sample checkbox", target: nil, action: nil)
  let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 280, height: 1200))
  text.isEditable = false
  text.string = (1...50).map { "Sample row \($0)" }.joined(separator: "\n")
  let scroll = NSScrollView()
  scroll.hasVerticalScroller = true
  scroll.autohidesScrollers = false
  scroll.documentView = text
  let other = NSButton(
    title: "Switch fixture window", target: switcher, action: #selector(Switcher.switchWindow(_:)))
  let duplicate = NSButton(
    title: "Match fixture windows", target: switcher, action: #selector(Switcher.matchWindows(_:)))
  let marker = NSView()
  marker.wantsLayer = true
  marker.layer?.backgroundColor =
    (index == 1
    ? NSColor(srgbRed: 0.9, green: 0.05, blue: 0.8, alpha: 1)
    : NSColor(srgbRed: 0.05, green: 0.8, blue: 0.85, alpha: 1)).cgColor
  marker.widthAnchor.constraint(equalToConstant: 80).isActive = true
  marker.heightAnchor.constraint(equalToConstant: 16).isActive = true
  let stack = NSStackView(views: [marker, toggle, field, secure, scroll, other, duplicate])
  stack.orientation = .vertical
  stack.alignment = .leading
  stack.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
  field.widthAnchor.constraint(equalToConstant: 280).isActive = true
  secure.widthAnchor.constraint(equalToConstant: 280).isActive = true
  scroll.widthAnchor.constraint(equalToConstant: 300).isActive = true
  scroll.heightAnchor.constraint(equalToConstant: 115).isActive = true
  window.contentView = stack
  window.makeKeyAndOrderFront(nil)
  switcher.windows.append(window)
}
app.activate(ignoringOtherApps: true)
app.run()
