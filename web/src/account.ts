import { createApp } from "vue";
import { initLang } from "./i18n";
import AccountCenter from "./AccountCenter.vue";
import "./ui/theme.css";

initLang();
createApp(AccountCenter).mount("#account");
