/** JSON events from the runtime. A blank line commits an event, even across network chunks. */
export async function* jsonEvents<T>(
  body: ReadableStream<Uint8Array>,
): AsyncGenerator<T> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let previousEndedInCR = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      let chunk = decoder.decode(value, { stream: true });
      if (!chunk) continue;
      if (previousEndedInCR && chunk.startsWith("\n")) chunk = chunk.slice(1);
      previousEndedInCR = chunk.endsWith("\r");
      const scanFrom = Math.max(0, buffer.length - 1);
      buffer += chunk.replace(/\r\n?/g, "\n");

      let cut = buffer.indexOf("\n\n", scanFrom);
      while (cut >= 0) {
        const payload = buffer.slice(0, cut).split("\n")
          .filter(line => line === "data" || line.startsWith("data:"))
          .map(line => line.slice(5).replace(/^ /, ""))
          .join("\n");
        if (payload) {
          const event: T | null = JSON.parse(payload);
          if (event !== null) yield event;
        }
        buffer = buffer.slice(cut + 2);
        cut = buffer.indexOf("\n\n");
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
