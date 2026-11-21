const invoke = window.__TAURI__.core.invoke;
const zh = navigator.language.startsWith("zh");
const text = (localized, english) => zh ? localized : english;
const messages = {
  offline: text("这台 Mac 尚未连接到 Jeeves。", "This Mac is disconnected from Jeeves."),
  connecting: text("正在连接…", "Connecting…"),
  pairing: text("在刚打开的浏览器中登录并确认连接。", "Sign in and confirm the connection in your browser."),
  online: text("Mac 已连接。Jeeves 会请你为任务选择窗口。", "Mac connected. Jeeves will ask you to select a window for a task."),
  executing: text("正在处理授权窗口中的操作。", "Working in the authorized window."),
  reconnecting: text("连接中断，正在重连…", "Reconnecting…"),
};
const status = document.querySelector("#status");
const connect = document.querySelector("#connect");
const disconnect = document.querySelector("#disconnect");
connect.textContent = text("连接这台 Mac", "Connect this Mac");
disconnect.textContent = text("停止并断开", "Stop and disconnect");
let error = "";
async function refresh() {
  try {
    const current = await invoke("device_status");
    status.textContent = error || messages[current];
    connect.hidden = current !== "offline";
    disconnect.hidden = current === "offline";
  } catch {
    status.textContent = text("无法读取连接状态。", "Could not read the device status.");
  }
}
connect.onclick = async () => {
  error = "";
  try { await invoke("connect"); }
  catch { error = text("连接失败，请检查网络后重试。", "Connection failed. Check the network and try again."); }
  await refresh();
};
disconnect.onclick = async () => { error = ""; await invoke("disconnect"); await refresh(); };
await refresh();
setInterval(refresh, 900);
