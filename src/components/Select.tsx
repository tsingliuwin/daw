import { createSignal, For, Show, onMount, onCleanup, type JSX } from "solid-js";
import { Portal } from "solid-js/web";

/**
 * 通用自定义下拉，替代原生 <select>。
 *
 * 原生 <select> 的下拉面板宽度/样式由操作系统控制（macOS 无法自定义、系统蓝高亮），
 * 本组件用 div 模拟，让面板宽度跟触发器等宽、配色跟随主题、消除系统蓝。
 *
 * 面板通过 Portal 渲染到 document.body 并用 position:fixed 定位（基于 trigger 的
 * getBoundingClientRect），使其脱离父级 overflow 容器（如设置页 .settings-content
 * 的 overflow-y:auto），避免被裁剪。
 *
 * 用法：
 *   <Select value="a" onChange={setV}
 *     options={[{ value: "a", label: "选项A" }, { value: "b", label: "选项B" }]} />
 */
export interface SelectOption {
  value: string;
  label: string;
  /** 可选的前置图标（SVG 节点）。 */
  icon?: JSX.Element;
}

export default function Select<T extends string>(props: {
  value: T;
  options: readonly SelectOption[];
  onChange: (v: T) => void;
  /** 触发器宽度，默认 180px。设为 "100%" 时撑满父级。 */
  width?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = createSignal(false);
  let wrapperRef!: HTMLDivElement;
  let triggerRef!: HTMLButtonElement;
  // 浮层 fixed 坐标（视口坐标），由 trigger 的 getBoundingClientRect 计算。
  const [panelStyle, setPanelStyle] = createSignal<JSX.CSSProperties>({});

  const selectedLabel = () =>
    props.options.find((o) => o.value === props.value)?.label ?? "";

  const selectedOption = () =>
    props.options.find((o) => o.value === props.value);

  /** 展开时根据 trigger 位置算出浮层坐标，默认向下展开，下方空间不足则向上。 */
  const positionPanel = () => {
    const el = triggerRef;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const panelHeight = props.options.length * 34 + 12; // 粗估：每项约 34px + padding
    const spaceBelow = window.innerHeight - r.bottom;
    const openDown = spaceBelow >= panelHeight || spaceBelow >= r.top;
    setPanelStyle({
      left: `${r.left}px`,
      width: `${r.width}px`,
      ...(openDown
        ? { top: `${r.bottom + 4}px` }
        : { bottom: `${window.innerHeight - r.top + 4}px` }),
    });
  };

  const toggle = () => {
    if (props.disabled) return;
    if (!open()) positionPanel();
    setOpen(!open());
  };

  onMount(() => {
    const onDocClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!open()) return;
      // 浮层渲染在 body，wrapperRef 只含 trigger；两者之外点击则收起。
      if (wrapperRef && !wrapperRef.contains(target) && !(target as Element).closest?.(".lm-select-list")) {
        setOpen(false);
      }
    };
    const onScrollOrResize = () => { if (open()) setOpen(false); };
    document.addEventListener("mousedown", onDocClick);
    window.addEventListener("resize", onScrollOrResize);
    window.addEventListener("scroll", onScrollOrResize, true);
    onCleanup(() => {
      document.removeEventListener("mousedown", onDocClick);
      window.removeEventListener("resize", onScrollOrResize);
      window.removeEventListener("scroll", onScrollOrResize, true);
    });
  });

  return (
    <div
      class="lm-select-wrapper"
      ref={wrapperRef}
      style={{ width: props.width ?? "180px" }}
    >
      <button
        type="button"
        ref={triggerRef}
        class="lm-select-trigger"
        classList={{ disabled: !!props.disabled }}
        disabled={props.disabled}
        onClick={toggle}
      >
        <Show when={selectedOption()?.icon}>
          <span class="lm-select-option-icon">{selectedOption()?.icon}</span>
        </Show>
        <span class="lm-select-label">{selectedLabel()}</span>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="lm-select-caret">
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </button>
      <Portal>
        <Show when={open()}>
          <div class="lm-select-list lm-select-floating" style={panelStyle()}>
            <For each={[...props.options]}>
              {(o) => (
                <button
                  type="button"
                  class="lm-select-item"
                  classList={{ active: o.value === props.value }}
                  onClick={() => {
                    props.onChange(o.value as T);
                    setOpen(false);
                  }}
                >
                  <Show when={o.icon}>
                    <span class="lm-select-option-icon">{o.icon}</span>
                  </Show>
                  {o.label}
                </button>
              )}
            </For>
          </div>
        </Show>
      </Portal>
    </div>
  );
}
