<script setup lang="ts">
import { ref, watch } from "vue";
import { responsibilityRequest } from "./api";
import { getLang } from "./i18n";
import type { ResponsibilityReview, ResponsibilityListResponse, ResponsibilityContract, ResponsibilityReportsResponse } from "./protocol";
const props = defineProps<{ busy: boolean; history?: boolean }>();
const reviews=ref<ResponsibilityReview[]>([]);
const duties=ref<ResponsibilityListResponse["responsibilities"]>([]);
const working=ref(false);
const error=ref("");
const samples=ref<Record<string,string>>({});
let generation=0;
async function load() {
  const stamp=++generation;
  try {
    const [pending,current]=await Promise.all([
      responsibilityRequest<ResponsibilityReview[]>("/reviews"),
      responsibilityRequest<ResponsibilityListResponse>(""),
    ]);
    if (stamp!==generation) return;
    reviews.value=pending;
    duties.value=current.responsibilities;
    error.value="";
  } catch(e) {if (stamp===generation) error.value=String(e);}
}
watch(()=>props.busy, busy=>{if (!busy) void load();},{immediate:true});
async function act(fn:()=>Promise<unknown>) {
  if (working.value) return;
  working.value=true;
  try {await fn(); await load();} catch(e) {error.value=String(e);} finally {working.value=false;}
}
function phrase(zh:string,en:string){return getLang()==="en"?en:zh;}
function cadence(cron:string){
  const [minute="",hour="",date="",month="",day=""]=cron.split(/\s+/);
  const names=phrase("日一二三四五六日","Sun,Mon,Tue,Wed,Thu,Fri,Sat,Sun");
  const name=(n:string)=>(getLang()==="en"?names.split(",")[Number(n)]:names[Number(n)])??n;
  if(date==="*" && month==="*" && /^\d+$/.test(minute) && /^\d+$/.test(hour)){
    const when=day==="*"?phrase("每天","Daily"):phrase("每周","Weekly ")+day.replace(/[0-7]/g,name);
    return when+" "+hour.padStart(2,"0")+":"+minute.padStart(2,"0");
  }
  return [phrase("分钟","Minute"),phrase("小时","Hour"),phrase("日期","Day"),phrase("月","Month"),phrase("星期","Weekday")].map((k,i)=>k+" "+(cron.split(/\s+/)[i]??"").replaceAll("*",phrase("每个","every"))).join(" · ");
}
function describe(c:ResponsibilityContract) {
  const labels:Record<string,string>={calendar:phrase("日历","Calendar"),openTasks:phrase("未完成事项","Open tasks"),unreadMail:phrase("未读邮件","Unread mail"),today:phrase("今天","Today"),tomorrow:phrase("明天","Tomorrow"),thisWeek:phrase("本周","This week"),nextWeek:phrase("下周","Next week")};
  return [cadence(c.cron), c.digest?.timeZone ?? phrase("主机当地时间","Runtime local time"),
    c.sources.map(s=>labels[s]??s).join("、"), c.digest && c.sources.includes("calendar")?labels[c.digest.calendarWindow]:null,
    c.digest?(c.digest.layout==="paragraphs"?phrase("每个来源一段","Paragraph per source"):phrase("列表","List")):null,
    c.weeklyReport?phrase("批准后发送给 ","Review before sending to ")+c.weeklyReport.recipient+" · "+c.weeklyReport.taskId:c.digest?phrase("发到主聊天","Deliver in main chat"):null,
    phrase("到期 ","Ends ")+new Date(c.expiresAt).toLocaleString(getLang())].filter(Boolean).join(" · ");
}
const actions:Record<string,string>={create:phrase("启用","Activate"),amend:phrase("确认修改","Apply change"),pause:phrase("暂停","Pause"),resume:phrase("恢复","Resume")};
async function inspect(id:string) {
  const result=await responsibilityRequest<ResponsibilityReportsResponse>("/reports?id="+encodeURIComponent(id));
  samples.value[id]=result.reports.map(r=>[r.status,r.body,r.detail].filter(Boolean).join("\n")).join("\n\n")||phrase("尚无运行记录","No runs yet");
}
</script>
<template>
  <section v-if="reviews.length || error || (history && duties.length)" class="duty-cards" :aria-label="phrase('周期委托','Recurring work')">
    <p v-if="error" role="status">{{ error }}</p>
    <article v-for="r in reviews" :key="r.id">
      <strong>{{ r.contract.purpose }}</strong>
      <p>{{ r.request }}</p>
      <p>{{ describe(r.contract) }}</p>
      <button v-if="r.contract.digest" :disabled="working" @click="act(async()=>{samples[r.id]=await responsibilityRequest<string>('/sample',r.contract)})">{{ phrase("预览一次","Preview once") }}</button>
      <button :disabled="working" @click="act(()=>responsibilityRequest('/decide',{id:r.id,accept:true}))">{{ actions[r.action] }}</button>
      <button :disabled="working" @click="act(()=>responsibilityRequest('/decide',{id:r.id,accept:false}))">{{ phrase("取消","Dismiss") }}</button>
      <pre v-if="samples[r.id]">{{ samples[r.id] }}</pre>
    </article>
    <template v-if="history">
      <article v-for="d in duties" :key="d.contract.id">
        <strong>{{ d.contract.purpose }}</strong>
        <p>{{ describe(d.contract) }} · {{ d.status==='active'?phrase("已启用","Active"):d.status==='paused'?phrase("已暂停","Paused"):phrase("已到期","Expired") }}</p>
        <button :disabled="working" @click="act(()=>inspect(d.contract.id))">{{ phrase("运行记录","Run history") }}</button>
        <pre v-if="samples[d.contract.id]">{{ samples[d.contract.id] }}</pre>
      </article>
    </template>
  </section>
</template>
<style scoped>
.duty-cards {max-width:48rem;margin:0 auto;width:100%}
article {padding:1rem;border:1px solid var(--line);border-radius:12px;margin:.5rem 0;overflow-wrap:anywhere}
button {margin-inline-end:.5rem}
pre {white-space:pre-wrap;font:inherit}
</style>
