import type { Page, CDPSession } from "playwright";
/** A separate JavaScript world prevents page scripts from rewriting the mutation counter. */
export class DocumentClock {
  private cdp: CDPSession;
  private constructor(cdp: CDPSession) {
    this.cdp = cdp;
  }
  static async create(page: Page) {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Page.enable");
    await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
      worldName: "jeeves-observation",
      source: `(()=>{let n=0;new MutationObserver(()=>n++).observe(document,{subtree:true,childList:true,attributes:true,characterData:true});Object.defineProperty(window,'__jeevesVersion',{get:()=>n});})()`,
    });
    return new DocumentClock(cdp);
  }
  async read() {
    const { frameTree } = await this.cdp.send("Page.getFrameTree");
    const { executionContextId } = await this.cdp.send(
      "Page.createIsolatedWorld",
      { frameId: frameTree.frame.id, worldName: "jeeves-observation" },
    );
    const result = await this.cdp.send("Runtime.evaluate", {
      contextId: executionContextId,
      expression: "window.__jeevesVersion ?? 0",
      returnByValue: true,
    });
    if (result.exceptionDetails)
      throw Error("Document changed during observation");
    const version = Number(result.result.value) || 0;
    return {
      token: `${frameTree.frame.loaderId}:${executionContextId}:${version}`,
      version: Math.min(version, 2147483647),
    };
  }
}
