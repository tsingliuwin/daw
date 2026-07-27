import MarkdownRenderer from "./MarkdownRenderer";

/**
 * 渲染一段纯文本（markdown）。aioa 没有 inline chart 引用，所以这里退化为
 * 直接用 MarkdownRenderer 渲染整段文本。保留组件名与 lakemind 一致，方便
 * ChatView 等上层引用。
 */
export default function MessageText(props: { text: string }) {
  return <MarkdownRenderer content={props.text} />;
}
