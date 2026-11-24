import UIKit
import WebKit

@main
final class WatchComposer: UIResponder, UIApplicationDelegate, WKNavigationDelegate {
    var window: UIWindow?
    let web = WKWebView()
    let env = ProcessInfo.processInfo.environment
    var submitted = false

    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let root = UIViewController()
        root.view = web
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = root
        window.makeKeyAndVisible()
        self.window = window
        web.navigationDelegate = self
        let url = URL(string: env["WATCH_WORKSPACE"]!)!
        let parts = env["WATCH_COOKIE"]!.split(separator: "=", maxSplits: 1).map(String.init)
        let cookie = HTTPCookie(properties: [.domain: url.host!, .path: "/", .name: parts[0], .value: parts[1]])!
        let script = WKUserScript(source: "localStorage.setItem('jeeves.lang','zh'); localStorage.setItem('jeeves.setup.dismissed','1');",
                                  injectionTime: .atDocumentStart, forMainFrameOnly: true)
        web.configuration.userContentController.addUserScript(script)
        web.configuration.websiteDataStore.httpCookieStore.setCookie(cookie) { self.web.load(URLRequest(url: url)) }
        return true
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard !submitted else { return }
        submitted = true
        let script = """
        async function until(find) {
            for (let i = 0; i < 200; i++) {
                const el = find(); if (el) return el;
                await new Promise(resolve => setTimeout(resolve, 100));
            }
            throw Error('Missing workspace control at ' + location.pathname);
        }
        (await until(() => [...document.querySelectorAll('nav button')].find(b => b.textContent.trim() === '工作'))).click();
        (await until(() => document.querySelector('[data-cowork-task="' + task + '"]'))).click();
        const input = await until(() => document.querySelector('#cowork-input'));
        const committed = new Promise((resolve,reject) => {
            const fetchRequest = window.fetch.bind(window);
            window.fetch = async (...args) => {
                const response = await fetchRequest(...args);
                if (response.url.endsWith('/api/interaction')) {
                    response.clone().text().then(body => {
                        if (body.includes('1700.00') && body.includes('"type":"turnCompleted"')) resolve();
                        else reject(Error('The Runtime did not confirm the new threshold'));
                    },reject);
                }
                return response;
            };
        });
        input.value = '改成低于1700才提醒，其他照旧。';
        input.dispatchEvent(new Event('input', {bubbles:true}));
        (await until(() => document.querySelector('#cowork-send:not([disabled])'))).click();
        await committed;
        return 'Submitted';
        """
        web.callAsyncJavaScript(script, arguments: ["task": env["WATCH_TASK"]!], in: nil, in: .page) { result in
            let path = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("result")
            let text: String
            switch result {
            case .success: text = "Submitted"
            case .failure(let error): text = error.localizedDescription
            }
            try? text.write(to: path, atomically: true, encoding: .utf8)
        }
    }
}
