import AppKit

@main struct WindowHelper {
  @MainActor static func run(_ scope: Scope, _ line: String) async -> Object {
    do {
      guard let p = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? Object else {
        throw Failure.denied("Invalid command")
      }
      switch p["kind"] as? String {
      case "select":
        try scope.select()
        return try scope.observe()
      case "observe": return try scope.observe()
      case "act": return try scope.act(p)
      case "frame":
        if #available(macOS 14.0, *) { return try await scope.frame(p) }
        throw Failure.denied("Window previews require macOS 14 or later")
      default: throw Failure.denied("Unknown window command")
      }
    } catch let failure as Failure { return failure.reply } catch {
      return Failure.denied("Native window service unavailable").reply
    }
  }
  @MainActor static func main() {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let scope = Scope()
    // Watch the parent pipe even during a modal dialog. A crashed Runtime cannot leave
    // a window chooser open or a native input queued for a later Runtime.
    let commands = AsyncStream<String>(bufferingPolicy: .bufferingOldest(1)) { queue in
      DispatchQueue.global().async {
        while let line = readLine() {
          guard case .enqueued = queue.yield(line) else { Darwin._exit(1) }
        }
        Darwin._exit(0)
      }
    }
    Task { @MainActor in
      for await line in commands {
        let reply = await run(scope, line)
        guard let data = try? JSONSerialization.data(withJSONObject: reply) else { break }
        FileHandle.standardOutput.write(data + Data([10]))
      }
      app.terminate(nil)
    }
    app.run()
  }
}
