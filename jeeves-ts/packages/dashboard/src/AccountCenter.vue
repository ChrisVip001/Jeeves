<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { ArrowRight, Laptop, Smartphone, Monitor } from "lucide-vue-next";
import FactorSettings from "./FactorSettings.vue";
import JeevesLogo from "./ui/JeevesLogo.vue";
import { getLang, setLang, t } from "./i18n";

interface Session {
  id: string;
  client: string;
  issued: number;
  expires: number;
}
interface Profile {
  email: string;
  factorEnabled: boolean;
  createdAt: string;
  app: string;
  used: number;
  quota: number | null;
  sessions: Session[];
  current: string;
}
const profile = ref<Profile>();
const email = ref("");
const password = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const signup = ref(false);
type Mode =
  "login" | "register" | "forgot" | "verify" | "reset" | "factor" | "mail";
const mode = ref<Mode>("login");
const token = ref("");
const code = ref("");
const notice = ref("");
const mailPath = ref("");
const mailLink = ref("");
function useMailLink() {
  try {
    const url = new URL(mailLink.value.trim());
    const fields = new URLSearchParams(url.hash.slice(1));
    const expected = mailPath.value === "/api/forgot" ? "reset" : "verify";
    if (url.origin !== location.origin || url.pathname !== "/account/" ||
        fields.get("email") !== email.value.trim().toLowerCase() ||
        fields.get("action") !== expected || !fields.get("token")) throw Error();
    location.hash = url.hash;
  } catch {
    error.value = t("请粘贴本次邮件中的完整链接。");
  }
}
function openMailLink() {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const action = fragment.get("action");
  if ((action === "verify" || action === "reset") && fragment.has("token")) {
    switchMode(action);
    token.value = fragment.get("token")!;
    email.value = fragment.get("email") ?? "";
    history.replaceState(null, "", location.pathname + location.search);
  }
}
function switchMode(next: Mode) {
  mode.value = next;
  mailLink.value = "";
  password.value = code.value = notice.value = error.value = token.value = "";
}
const ready = ref(false);
const busy = ref(false);
const error = ref("");
const lang = getLang();
const date = (value: string | number) => new Date(value).toLocaleString(lang);
const app = computed(() => {
  const base = profile.value?.app ?? "";
  const next = new URLSearchParams(location.search).get("next") ?? "";
  return base && (next === base || next.startsWith(base + "?")) ? next : base;
});
async function refresh() {
  const response = await fetch("/api/account", { cache: "no-store" });
  if (response.status === 401) {
    profile.value = undefined;
    currentPassword.value = newPassword.value = "";
    signup.value = (await response.json()).signup === true;
  } else if (response.ok) profile.value = await response.json();
  else throw Error(t("无法读取账号，请重试。"));
  ready.value = true;
}
async function act(action: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    await action();
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("连接失败，请重试。");
  } finally {
    busy.value = false;
  }
}
async function requestMail(path: string) {
  const response = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: email.value }),
  });
  if (!response.ok) throw Error(t("发送失败，请稍后重试。"));
  mailPath.value = path;
  mode.value = "mail";
  notice.value = t(
    "如果邮箱符合条件，你将收到一封邮件。请在 15 分钟内打开链接。",
  );
}
function submit() {
  void act(async () => {
    const step = mode.value;
    if (step === "register" || step === "forgot" || step === "mail")
      return requestMail(
        step === "mail"
          ? mailPath.value
          : step === "register"
            ? "/api/register"
            : "/api/forgot",
      );
    const path =
      step === "reset" || step === "verify" ? "/api/" + step : "/api/login";
    const response = await fetch(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        email: email.value,
        password: password.value,
        token: token.value,
        code: code.value,
      }),
    });
    if (!response.ok)
      throw Error(
        response.status === 429
          ? t("尝试过于频繁，请稍后重试。")
          : step === "verify" || step === "reset"
            ? t("请检查密码和邮件链接，或重新申请邮件。")
            : step === "factor"
              ? t("验证码无效或已使用。")
              : t("登录失败，请检查邮箱和密码。"),
      );
    const result = await response.json();
    if (result.step === "factor") {
      mode.value = "factor";
      return;
    }
    if (result.step === "verify") {
      password.value = "";
      return requestMail("/api/send-verification");
    }
    switchMode("login");
    if (step === "reset") notice.value = t("密码已重设，请重新登录。");
    await refresh();
    if (location.search && profile.value) location.replace(app.value);
  });
}
function revoke(id: string) {
  void act(async () => {
    const current = id === profile.value?.current;
    const response = await fetch(
      current
        ? "/api/logout"
        : "/api/account/sessions/" + encodeURIComponent(id),
      {
        method: current ? "POST" : "DELETE",
        headers: { "x-jeeves-client": "1" },
      },
    );
    if (!response.ok && response.status !== 401)
      throw Error(t("退出失败，请重试。"));
    if (current) {
      email.value = "";
      switchMode("login");
    }
    await refresh();
  });
}
function changePassword() {
  void act(async () => {
    const response = await fetch("/api/account/password", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        current: currentPassword.value,
        replacement: newPassword.value,
      }),
    });
    if (!response.ok && response.status !== 401)
      throw Error(t("密码未更新，请检查当前密码或稍后重试。"));
    switchMode("login");
    await refresh();
  });
}
function restored(event: PageTransitionEvent) {
  if (event.persisted) void act(refresh);
}
onMounted(() => {
  openMailLink();
  window.addEventListener("hashchange", openMailLink);
  void act(refresh);
  window.addEventListener("pageshow", restored);
});
onUnmounted(() => {
  window.removeEventListener("hashchange", openMailLink);
  window.removeEventListener("pageshow", restored);
});
</script>
<template>
  <header>
    <div class="brand"><JeevesLogo :size="34" /> Jeeves</div>
    <button class="quiet" @click="setLang(lang === 'en' ? 'zh' : 'en')">
      {{ lang === "en" ? "中文" : "English" }}
    </button>
  </header>
  <section v-if="profile && mode === 'login'">
    <h1>{{ t("个人中心") }}</h1>
    <p class="identity">{{ profile.email }}</p>
    <a id="account-workspace" class="primary" :href="app"
      >{{ t("打开我的助理") }} <ArrowRight :size="16"
    /></a>
    <dl>
      <div>
        <dt>{{ t("注册时间") }}</dt>
        <dd>{{ date(profile.createdAt) }}</dd>
      </div>
      <div v-if="profile.quota !== null">
        <dt>{{ t("本月模型用量") }}</dt>
        <dd>
          {{ profile.used.toLocaleString() }} /
          {{ profile.quota.toLocaleString() }} tokens
        </dd>
      </div>
    </dl>
    <FactorSettings
      :key="profile.email"
      :enabled="profile.factorEnabled"
      @updated="refresh"
    />
    <h2>{{ t("登录会话") }}</h2>
    <ul>
      <li v-for="s in profile.sessions" :key="s.id">
        <component
          :is="
            s.client === 'iOS'
              ? Smartphone
              : s.client === 'Mac'
                ? Laptop
                : Monitor
          "
          :size="21"
        />
        <div>
          <strong>{{ s.client }}</strong>
          <small v-if="s.id === profile.current">{{ t("当前会话") }}</small>
          <p>{{ date(s.issued) }}</p>
        </div>
        <button
          :id="s.id === profile.current ? 'account-logout' : undefined"
          class="quiet"
          :disabled="busy"
          @click="revoke(s.id)"
        >
          {{ t("退出登录") }}
        </button>
      </li>
    </ul>
    <details>
      <summary>{{ t("修改密码") }}</summary>
      <form @submit.prevent="changePassword">
        <label
          >{{ t("当前密码")
          }}<input
            v-model="currentPassword"
            required
            type="password"
            autocomplete="current-password"
        /></label>
        <label
          >{{ t("新密码")
          }}<input
            v-model="newPassword"
            required
            minlength="10"
            type="password"
            autocomplete="new-password"
        /></label>
        <small>{{ t("修改密码后，所有设备都需重新登录。") }}</small>
        <button class="primary" :disabled="busy">{{ t("更新密码") }}</button>
      </form>
    </details>
  </section>
  <section v-else-if="ready">
    <h1>
      {{
        mode === "register"
          ? t("创建 Jeeves 账号")
          : mode === "forgot" || mode === "reset"
            ? t("找回密码")
            : mode === "verify"
              ? t("验证邮箱")
              : mode === "factor"
                ? t("双因素认证")
                : mode === "mail"
                  ? t("查看邮件")
                  : t("登录 Jeeves")
      }}
    </h1>
    <p v-if="notice" role="status">{{ notice }}</p>
    <form v-if="mode === 'mail'" @submit.prevent="useMailLink">
      <label>{{ t("在此窗口打开邮件链接") }}<input id="account-mail-link" v-model="mailLink" type="url" autocomplete="off" required /></label>
      <button id="account-open-link" class="primary">{{ t("打开邮件链接") }}</button>
    </form>
    <form @submit.prevent="submit">
      <label v-if="mode !== 'mail' && mode !== 'factor'"
        >{{ t("邮箱地址")
        }}<input
          id="account-email"
          v-model="email"
          type="email"
          :readonly="mode === 'verify' || mode === 'reset'"
          autocomplete="username"
          required
      /></label>
      <small v-if="mode === 'verify' || mode === 'reset'">{{
        t("密码至少 10 个字符。")
      }}</small>
      <small v-if="mode === 'verify'">{{
        t("新账号请设置密码，已有账号请输入原密码。")
      }}</small>
      <label v-if="['login', 'reset', 'verify'].includes(mode)"
        >{{ mode === "reset" ? t("新密码") : t("密码")
        }}<input
          id="account-password"
          v-model="password"
          type="password"
          :autocomplete="mode === 'login' ? 'current-password' : 'new-password'"
          :minlength="mode === 'login' ? 1 : 10"
          required
      /></label>
      <label v-if="mode === 'factor'"
        >{{ t("验证码或恢复码")
        }}<input
          id="account-code"
          v-model="code"
          autocomplete="one-time-code"
          required
      /></label>
      <button id="account-submit" class="primary" :disabled="busy">
        {{
          mode === "register"
            ? t("创建账号")
            : mode === "forgot"
              ? t("发送重设邮件")
              : mode === "mail"
                ? t("重新发送")
                : mode === "verify"
                  ? t("验证邮箱")
                  : mode === "reset"
                    ? t("更新密码")
                    : t("登录")
        }}
      </button>
    </form>
    <button
      v-if="mode !== 'login'"
      class="quiet switch"
      @click="switchMode('login')"
      :disabled="busy"
    >
      {{ t("返回登录") }}
    </button>
    <button
      id="account-register"
      v-if="signup && mode === 'login'"
      class="quiet switch"
      @click="switchMode('register')"
      :disabled="busy"
    >
      {{ t("注册新账号") }}
    </button>
    <button
      v-if="mode === 'login'"
      class="quiet switch"
      id="account-forgot"
      @click="switchMode('forgot')"
      :disabled="busy"
    >
      {{ t("找回密码") }}
    </button>
  </section>
  <p v-if="error" role="alert">
    {{ error }}
    <button v-if="!ready" class="quiet" @click="act(refresh)">
      {{ t("重试") }}
    </button>
  </p>
</template>
<style>
:root {
  font:
    16px/1.5 system-ui,
    sans-serif;
  color: #22332b;
  background: #f5f6f2;
  color-scheme: light;
}
* {
  box-sizing: border-box;
}
body {
  margin: 0;
}
main {
  max-width: 760px;
  margin: auto;
  padding: 0 24px 48px;
}
header,
.brand,
.primary,
li {
  display: flex;
  align-items: center;
  gap: 12px;
}
header {
  justify-content: space-between;
  padding: 27px 0;
  border-bottom: 1px solid #d4dcd4;
}
.brand {
  font-size: 21px;
  font-weight: 650;
}
section {
  max-width: 490px;
  margin: clamp(32px, 8vh, 76px) auto 0;
}
h1 {
  font-size: clamp(30px, 5vw, 42px);
  line-height: 1.15;
  letter-spacing: -0.04em;
  margin: 0 0 22px;
}
h2 {
  font-size: 21px;
  margin: 32px 0 8px;
}
.identity {
  font-size: 18px;
  margin: -9px 0 24px;
}
form,
label {
  display: grid;
  gap: 6px;
}
form {
  gap: 17px;
}
input,
button,
a {
  font: inherit;
}
input {
  padding: 11px;
  border: 1px solid #9eaca1;
  border-radius: 6px;
  background: #fff;
  color: inherit;
}
button,
a {
  cursor: pointer;
}
button:disabled {
  opacity: 0.55;
}
.primary {
  justify-content: center;
  background: #23583d;
  color: white;
  text-decoration: none;
  font-weight: 600;
  border: 0;
  border-radius: 7px;
  min-height: 44px;
  padding: 9px 18px;
}
.quiet {
  background: none;
  border: 0;
  color: #28583f;
  text-decoration: underline;
  text-underline-offset: 3px;
  min-height: 44px;
}
.switch {
  margin-top: 14px;
}
dl {
  margin-top: 28px;
}
dl div,
li {
  border-bottom: 1px solid #d4dcd4;
  padding: 13px 0;
}
dl div {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
}
dd {
  margin: 0;
}
li p,
small,
dt {
  color: #506457;
}
li p {
  margin: 0;
  font-size: 14px;
}
ul {
  list-style: none;
  padding: 0;
  margin: 0;
}
li button {
  margin-left: auto;
}
p[role="alert"] {
  color: #942e24;
  max-width: 490px;
  margin: 16px auto;
}
button:focus-visible,
a:focus-visible,
input:focus-visible {
  outline: 3px solid #36714e;
  outline-offset: 3px;
}
</style>
