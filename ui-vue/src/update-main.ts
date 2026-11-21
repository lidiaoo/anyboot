import { createApp, h } from "vue";
import UpdateDialog from "@/components/update/UpdateDialog.vue";

const app = createApp({
  render: () => h(UpdateDialog),
});
app.mount("#app");
