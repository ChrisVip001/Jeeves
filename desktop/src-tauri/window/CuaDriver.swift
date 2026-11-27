import AppKit
import CryptoKit
import ImageIO
import Security
import Vision

final class CuaDriver {
  private static let bundle = URL(fileURLWithPath: "/Applications/CuaDriver.app")
  private let process = Process()
  private let input = Pipe()
  private let output = Pipe()
  private var buffer = Data()
  private var sequence = 0

  static var installed: Bool { FileManager.default.fileExists(atPath: bundle.path) }

  init() throws {
    var code: SecStaticCode?
    var requirement: SecRequirement?
    let flags = SecCSFlags(rawValue: 0)
    let signer = """
      identifier "com.trycua.driver" and anchor apple generic and certificate leaf[subject.OU] = "YCK386LBJ7"
      """
    guard SecStaticCodeCreateWithPath(Self.bundle as CFURL, flags, &code) == errSecSuccess,
      SecRequirementCreateWithString(signer as CFString, flags, &requirement) == errSecSuccess,
      let code, let requirement,
      SecStaticCodeCheckValidity(code, SecCSFlags(rawValue: kSecCSStrictValidate), requirement)
        == errSecSuccess
    else { throw Failure.denied("Install the signed Cua Driver app in /Applications") }
    process.executableURL = Self.bundle.appendingPathComponent("Contents/MacOS/cua-driver")
    process.arguments = ["mcp", "--direct", "--embedded"]
    process.standardInput = input
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    try process.run()
    _ = try request("initialize", [
      "protocolVersion": "2024-11-05", "capabilities": Object(),
      "clientInfo": ["name": "Jeeves", "version": "1"],
    ])
    try write(["jsonrpc": "2.0", "method": "notifications/initialized"])
    let permissions = try call("check_permissions", [:]).fields
    guard permissions["accessibility"] as? Bool == true,
      permissions["screen_recording"] as? Bool == true
    else {
      throw Failure.denied(
        "Allow Accessibility and Screen Recording for Jeeves in System Settings, then select again")
    }
  }

  deinit {
    try? input.fileHandleForWriting.close()
    if process.isRunning { process.terminate() }
  }

  private func write(_ message: Object) throws {
    guard process.isRunning else { throw Failure.reselect("Cua Driver stopped; select again") }
    let data = try JSONSerialization.data(withJSONObject: message)
    try input.fileHandleForWriting.write(contentsOf: data + Data([10]))
  }

  private func request(_ method: String, _ params: Object) throws -> Object {
    sequence += 1
    try write(["jsonrpc": "2.0", "id": sequence, "method": method, "params": params])
    let end = ContinuousClock.now + .seconds(4)
    let fd = output.fileHandleForReading.fileDescriptor
    var bytes = [UInt8](repeating: 0, count: 64 * 1024)
    while ContinuousClock.now < end {
      while let newline = buffer.firstIndex(of: 10) {
        let line = buffer[..<newline]
        buffer.removeSubrange(...newline)
        guard let message = try JSONSerialization.jsonObject(with: line) as? Object,
          message["jsonrpc"] as? String == "2.0"
        else { throw Failure.reselect("Invalid response from Cua Driver; select again") }
        guard message["id"] as? Int == sequence else { continue }
        guard let result = message["result"] as? Object else {
          throw Failure.denied("Cua Driver rejected the request")
        }
        return result
      }
      var source = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
      switch poll(&source, 1, 100) {
      case 0: continue
      case -1 where errno == EINTR: continue
      case 1:
        let count = bytes.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
        guard count > 0, buffer.count + count <= 6_000_000 else {
          throw Failure.reselect("Cua Driver stopped or returned too much data; select again")
        }
        buffer.append(contentsOf: bytes.prefix(count))
      default: throw Failure.reselect("Cua Driver stopped; select again")
      }
    }
    throw Failure.reselect("Cua Driver did not respond; select again")
  }

  func call(_ name: String, _ arguments: Object) throws -> (fields: Object, content: [Object]) {
    let reply = try request("tools/call", ["name": name, "arguments": arguments])
    guard reply["isError"] as? Bool != true, let fields = reply["structuredContent"] as? Object
    else { throw Failure.denied("Cua Driver could not complete \(name)") }
    return (fields, reply["content"] as? [Object] ?? [])
  }

  func windows(pid: pid_t) throws -> [CuaWindow] {
    try windowEntries(pid: pid).compactMap { entry in
      guard entry["pid"] as? Int == Int(pid), let id = entry["window_id"] as? UInt32, id > 0,
        let title = entry["title"] as? String, !title.isEmpty
      else { return nil }
      return CuaWindow(driver: self, pid: pid, id: id, title: title)
    }
  }

  func windowEntries(pid: pid_t) throws -> [Object] {
    let reply = try call("list_windows", ["pid": pid])
    return reply.fields["windows"] as? [Object] ?? []
  }
}

private struct CuaFrame {
  let image: CGImage
  let captureID: String
  let digest: SHA256.Digest
  let bounds: CGRect

  func screenPoint(_ rect: CGRect) -> CGPoint {
    CGPoint(
      x: bounds.minX + rect.midX * bounds.width / CGFloat(image.width),
      y: bounds.minY + rect.midY * bounds.height / CGFloat(image.height))
  }
}

struct VisualControl {
  let label: String
  let rect: CGRect

  static func detect(in image: CGImage) throws -> [Self] {
    let request = VNRecognizeTextRequest()
    request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"]
    request.usesLanguageCorrection = false
    for (stage, devices) in try request.supportedComputeStageDevices {
      let cpu = devices.first { device in
        if case .cpu = device { return true }
        return false
      }
      if let cpu { request.setComputeDevice(cpu, for: stage) }
    }
    try VNImageRequestHandler(cgImage: image).perform([request])
    let frame = CGRect(x: 0, y: 0, width: image.width, height: image.height)
    return (request.results ?? []).compactMap { line in
      guard let candidate = line.topCandidates(1).first, candidate.confidence >= 0.5,
        !candidate.string.isEmpty
      else { return nil }
      let b = line.boundingBox
      let rect = CGRect(
        x: b.minX * frame.width, y: (1 - b.maxY) * frame.height,
        width: b.width * frame.width, height: b.height * frame.height
      ).integral.intersection(frame)
      guard !rect.isEmpty else { return nil }
      return Self(label: String(candidate.string.prefix(160)), rect: rect)
    }
  }
}

final class CuaWindow {
  let title: String
  let id: UInt32
  private let driver: CuaDriver
  private let application: NSRunningApplication?
  private let pid: pid_t
  private var observation = ""
  private var controls: [VisualControl] = []
  private var digest: SHA256.Digest?

  init(driver: CuaDriver, pid: pid_t, id: UInt32, title: String) {
    self.driver = driver
    self.pid = pid
    self.id = id
    self.title = title
    application = NSRunningApplication(processIdentifier: pid)
  }

  private func capture() throws -> CuaFrame {
    guard let application, !application.isTerminated else {
      throw Failure.reselect("Selected application closed; select again")
    }
    let reply = try driver.call("get_window_state", [
      "pid": pid, "window_id": id, "include_accessibility_tree": false,
    ])
    let state = reply.fields
    guard state["pid"] as? Int == Int(pid), state["window_id"] as? UInt32 == id,
      state["window_title"] as? String == title
    else { throw Failure.reselect("Selected window changed; select it again") }
    guard state["screenshot_frame_valid"] as? Bool == true,
      let captureID = state["capture_id"] as? String, !captureID.isEmpty,
      let block = reply.content.first(where: { $0["type"] as? String == "image" }),
      let base64 = block["data"] as? String, let data = Data(base64Encoded: base64),
      let source = CGImageSourceCreateWithData(data as CFData, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
      image.width == state["screenshot_width"] as? Int,
      image.height == state["screenshot_height"] as? Int,
      let bounds = state["window_bounds"] as? Object,
      let x = bounds["x"] as? Double, let y = bounds["y"] as? Double,
      let width = bounds["width"] as? Double, let height = bounds["height"] as? Double,
      width > 0, height > 0
    else { throw Failure.denied("No usable image of the selected window; bring it into view") }
    return CuaFrame(
      image: image, captureID: captureID, digest: SHA256.hash(data: data),
      bounds: CGRect(x: x, y: y, width: width, height: height))
  }

  private func observe(_ frame: CuaFrame) throws -> Object {
    let found = try VisualControl.detect(in: frame.image)
    guard !found.isEmpty else {
      throw Failure.denied("No visible text found in the selected window; bring it into view")
    }
    observation = UUID().uuidString
    digest = frame.digest
    controls = Array(found.prefix(128))
    let items: [Object] = controls.enumerated().map { index, item in
      [
        "id": index, "role": "visualText", "label": item.label, "actions": ["click"],
        "bounds": [
          "x": Int(item.rect.minX), "y": Int(item.rect.minY),
          "width": Int(item.rect.width), "height": Int(item.rect.height),
        ],
      ]
    }
    return [
      "kind": "observed",
      "observation": [
        "observationId": observation, "window": String(title.prefix(256)),
        "elements": items, "truncated": found.count > controls.count,
      ] as Object,
    ]
  }

  func observe() throws -> Object {
    observation = ""
    return try observe(capture())
  }

  private func windowEntry() throws -> Object {
    guard let entry = try driver.windowEntries(pid: pid).first(where: {
      $0["pid"] as? Int == Int(pid) && $0["window_id"] as? UInt32 == id
        && $0["title"] as? String == title
    }) else { throw Failure.reselect("Selected window changed; select it again") }
    return entry
  }

  private func isVisible(_ entry: Object) -> Bool {
    entry["on_current_space"] as? Bool == true && entry["is_on_screen"] as? Bool == true
  }

  private func prepareClick() throws -> String {
    let entry = try windowEntry()
    let ordinary = entry["layer"] as? Int == 0
    if isVisible(entry) {
      if ordinary { return "background" }
      if application?.isActive == true { return "foreground" }
    }

    var target: Object = ["pid": pid]
    if ordinary { target["window_id"] = id }
    _ = try driver.call("bring_to_front", target)
    let deadline = ContinuousClock.now + .seconds(1)
    repeat {
      if try isVisible(windowEntry()) { return "foreground" }
      Thread.sleep(forTimeInterval: 0.1)
    } while ContinuousClock.now < deadline
    throw Failure.denied("Selected window did not become visible; bring it forward and observe again")
  }

  private func confirmForegroundPoint(_ point: CGPoint) throws {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [Object]
    let hit = windows?.first { entry in
      guard let alpha = entry[kCGWindowAlpha as String] as? Double, alpha > 0,
        let raw = entry[kCGWindowBounds as String] as? Object,
        let bounds = CGRect(dictionaryRepresentation: raw as CFDictionary)
      else { return false }
      return bounds.contains(point)
    }
    guard hit?[kCGWindowNumber as String] as? UInt32 == id,
      hit?[kCGWindowOwnerPID as String] as? Int == Int(pid)
    else { throw Failure.denied("Selected window is covered at the click point; observe again") }
  }

  func act(_ command: Object) throws -> Object {
    guard command["observationId"] as? String == observation, let index = command["elementId"] as? Int,
      controls.indices.contains(index), command["action"] as? String == "click",
      let digest
    else { throw Failure.denied("Choose a click from a fresh window observation") }
    observation = ""
    let delivery = try prepareClick()
    let before = try capture()
    guard before.digest == digest else { throw Failure.denied("Window image changed; observe again") }
    let point = controls[index].rect
    if delivery == "foreground" { try confirmForegroundPoint(before.screenPoint(point)) }
    do {
      _ = try driver.call("click", [
        "pid": pid, "window_id": id, "x": point.midX, "y": point.midY,
        "capture_id": before.captureID, "delivery_mode": delivery,
      ])
      let after = try capture()
      if after.digest != before.digest { return try observe(after) }
    } catch {
      throw Failure.uncertain("Window could not confirm the click; inspect the result before retrying")
    }
    throw Failure.uncertain("Window did not show a change; inspect the result before retrying")
  }

  func frame(_ command: Object) throws -> Object {
    guard command["observationId"] as? String == observation, digest != nil else {
      throw Failure.denied("Preview requires a fresh observation")
    }
    let frame = try capture()
    guard let jpeg = NSBitmapImageRep(cgImage: frame.image).representation(
      using: .jpeg, properties: [.compressionFactor: 0.7]), jpeg.count <= 800_000
    else { throw Failure.denied("Window preview exceeds the size limit") }
    return ["kind": "frame", "jpeg": jpeg.base64EncodedString()]
  }
}
