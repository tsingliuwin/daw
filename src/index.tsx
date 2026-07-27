/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import "./App.css";

// 启动入口：渲染根组件到 #root。
// 渲染完成后显式移除 index.html 里的 splash 占位，避免它在 App 异常或
// 样式未覆盖时一直转圈（splash 是 position:fixed，会盖在所有内容之上）。
const root = document.getElementById("root") as HTMLElement;
render(() => <App />, root);
// render 已把 App 挂载到 #root；此时 splash 仍是 #root 的兄弟节点（在挂载点
// 之外），手动清掉它。
document.querySelectorAll(".app-splash").forEach((el) => el.remove());
