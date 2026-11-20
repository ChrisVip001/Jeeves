import { createApp } from "vue";
import { initLang } from "./i18n";
import WebControls from "./WebControls.vue";
import "./ui/style.css";
import "./ui/today.css";
import "./style.css";
import "./workspace";

initLang();
createApp(WebControls).mount("#web-controls");
