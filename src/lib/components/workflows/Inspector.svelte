<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { CatalogEntry, WfNode } from "$lib/workflows/api";
  import { categoryColor, nodeIcon, CONDITION_OPS, PARAM_FORMS, SET_OPS, type Field } from "$lib/workflows/model";

  let {
    node,
    spec,
    onchange,
    ondelete,
  }: {
    node: WfNode;
    spec: CatalogEntry;
    onchange: () => void;
    ondelete: () => void;
  } = $props();

  const fields = $derived(PARAM_FORMS[node.type] ?? []);
  const color = $derived(categoryColor(spec.category));
  let jsonDrafts = $state<Record<string, string>>({});
  let jsonBad = $state<Record<string, boolean>>({});

  function get(key: string): unknown {
    return node.params[key];
  }

  function set(key: string, value: unknown) {
    node.params[key] = value;
    onchange();
  }

  function str(key: string): string {
    const v = get(key);
    return v == null ? "" : String(v);
  }

  function num(key: string): number | "" {
    const v = get(key);
    return typeof v === "number" ? v : "";
  }

  function setNum(key: string, raw: string) {
    const n = Number(raw);
    set(key, raw === "" || Number.isNaN(n) ? 0 : n);
  }

  function jsonText(key: string): string {
    if (key in jsonDrafts) return jsonDrafts[key];
    const v = get(key);
    return v == null ? "" : JSON.stringify(v, null, 2);
  }

  function setJson(key: string, raw: string) {
    jsonDrafts[key] = raw;
    if (raw.trim() === "") {
      jsonBad[key] = false;
      set(key, null);
      return;
    }
    try {
      set(key, JSON.parse(raw));
      jsonBad[key] = false;
    } catch {
      jsonBad[key] = true;
    }
  }

  // --- keyvalue -------------------------------------------------------------
  function kvRows(key: string): [string, string][] {
    const v = get(key);
    if (v && typeof v === "object" && !Array.isArray(v)) {
      return Object.entries(v as Record<string, unknown>).map(([k, val]) => [k, String(val ?? "")]);
    }
    return [];
  }

  function setKv(key: string, rows: [string, string][]) {
    const obj: Record<string, string> = {};
    for (const [k, v] of rows) {
      if (k.trim() !== "") obj[k] = v;
    }
    set(key, obj);
  }

  function kvEdit(key: string, index: number, which: 0 | 1, value: string) {
    const rows = kvRows(key);
    rows[index] = which === 0 ? [value, rows[index][1]] : [rows[index][0], value];
    setKv(key, rows);
  }

  // --- list-of-object params (conditions, ops) --------------------------------
  function rows(key: string): Record<string, unknown>[] {
    const v = get(key);
    return Array.isArray(v) ? (v as Record<string, unknown>[]) : [];
  }

  function pushRow(key: string, row: Record<string, unknown>) {
    set(key, [...rows(key), row]);
  }

  function dropRow(key: string, index: number) {
    set(
      key,
      rows(key).filter((_, i) => i !== index),
    );
  }

  function editRow(key: string, index: number, field: string, value: unknown) {
    const list = rows(key).map((r, i) => (i === index ? { ...r, [field]: value } : r));
    set(key, list);
  }

  function visible(field: Field): boolean {
    return !field.when || field.when(node.params);
  }
</script>

<div class="inspector">
  <div class="head" style="--node-color: {color}">
    <span class="head-icon"><Icon name={nodeIcon(node.type)} size={15} /></span>
    <div class="head-text">
      <input class="name-input" bind:value={node.name} oninput={onchange} spellcheck="false" />
      <div class="head-kind">{spec.label}</div>
    </div>
    <button class="btn btn-ghost btn-icon" title="Delete node" onclick={ondelete}>
      <Icon name="trash" size={14} />
    </button>
  </div>

  {#if spec.description}
    <p class="desc">{spec.description}</p>
  {/if}

  <div class="section">
    {#each fields as field (field.key + field.kind)}
      {#if visible(field)}
        <label class="field">
          <span class="field-label">{field.label}</span>
          {#if field.kind === "text"}
            <input value={str(field.key)} placeholder={field.placeholder ?? ""} spellcheck="false"
              oninput={(e) => set(field.key, e.currentTarget.value)} />
          {:else if field.kind === "expression"}
            <textarea rows="2" class="expr" value={str(field.key)} placeholder={field.placeholder ?? ""}
              spellcheck="false" oninput={(e) => set(field.key, e.currentTarget.value)}></textarea>
          {:else if field.kind === "textarea"}
            <textarea rows="4" value={str(field.key)} placeholder={field.placeholder ?? ""} spellcheck="false"
              oninput={(e) => set(field.key, e.currentTarget.value)}></textarea>
          {:else if field.kind === "code"}
            <textarea rows="10" class="code" value={str(field.key)} spellcheck="false"
              oninput={(e) => set(field.key, e.currentTarget.value)}></textarea>
          {:else if field.kind === "json"}
            <textarea rows="4" class="code" class:bad={jsonBad[field.key]} value={jsonText(field.key)}
              spellcheck="false" oninput={(e) => setJson(field.key, e.currentTarget.value)}></textarea>
            {#if jsonBad[field.key]}<span class="field-help bad-note">Not valid JSON yet</span>{/if}
          {:else if field.kind === "number"}
            <input type="number" value={num(field.key)} oninput={(e) => setNum(field.key, e.currentTarget.value)} />
          {:else if field.kind === "toggle"}
            <span class="toggle-row">
              <input type="checkbox" checked={Boolean(get(field.key))}
                onchange={(e) => set(field.key, e.currentTarget.checked)} />
              <span class="toggle-hint">{Boolean(get(field.key)) ? "On" : "Off"}</span>
            </span>
          {:else if field.kind === "select"}
            <select value={String(get(field.key) ?? "")}
              onchange={(e) => {
                const raw = e.currentTarget.value;
                const opt = field.options?.find((o) => String(o.value) === raw);
                set(field.key, opt ? opt.value : raw);
              }}>
              {#each field.options ?? [] as opt (String(opt.value))}
                <option value={String(opt.value)}>{opt.label}</option>
              {/each}
            </select>
          {:else if field.kind === "keyvalue"}
            <div class="kv">
              {#each kvRows(field.key) as [k, v], i (i)}
                <div class="kv-row">
                  <input value={k} placeholder="name" spellcheck="false"
                    oninput={(e) => kvEdit(field.key, i, 0, e.currentTarget.value)} />
                  <input value={v} placeholder="value" spellcheck="false"
                    oninput={(e) => kvEdit(field.key, i, 1, e.currentTarget.value)} />
                  <button class="row-x" onclick={() => setKv(field.key, kvRows(field.key).filter((_, x) => x !== i))}>
                    <Icon name="close" size={11} />
                  </button>
                </div>
              {/each}
              <button class="btn btn-ghost btn-sm add-row" onclick={() => setKv(field.key, [...kvRows(field.key), ["", ""]])}>
                <Icon name="plus" size={12} /> Add
              </button>
            </div>
          {:else if field.kind === "conditions"}
            <div class="kv">
              {#each rows(field.key) as row, i (i)}
                <div class="cond-row">
                  <input class="cond-left" value={String(row.left ?? "")} placeholder={'{{ $json.field }}'}
                    spellcheck="false" oninput={(e) => editRow(field.key, i, "left", e.currentTarget.value)} />
                  <select value={String(row.op ?? "eq")}
                    onchange={(e) => editRow(field.key, i, "op", e.currentTarget.value)}>
                    {#each CONDITION_OPS as op (op.value)}
                      <option value={op.value}>{op.label}</option>
                    {/each}
                  </select>
                  {#if row.op !== "exists" && row.op !== "notexists"}
                    <input value={String(row.right ?? "")} placeholder="value" spellcheck="false"
                      oninput={(e) => editRow(field.key, i, "right", e.currentTarget.value)} />
                  {/if}
                  <button class="row-x" onclick={() => dropRow(field.key, i)}><Icon name="close" size={11} /></button>
                </div>
              {/each}
              <button class="btn btn-ghost btn-sm add-row"
                onclick={() => pushRow(field.key, { left: "", op: "eq", right: "" })}>
                <Icon name="plus" size={12} /> Add condition
              </button>
            </div>
          {:else if field.kind === "ops"}
            <div class="kv">
              {#each rows(field.key) as row, i (i)}
                <div class="op-row">
                  <div class="op-head">
                    <select value={String(row.op ?? "set")}
                      onchange={(e) => editRow(field.key, i, "op", e.currentTarget.value)}>
                      {#each SET_OPS as op (op.value)}
                        <option value={op.value}>{op.label}</option>
                      {/each}
                    </select>
                    <button class="row-x" onclick={() => dropRow(field.key, i)}><Icon name="close" size={11} /></button>
                  </div>
                  {#if row.op === "keep"}
                    <input value={Array.isArray(row.paths) ? (row.paths as string[]).join(", ") : ""}
                      placeholder="fields to keep, comma separated" spellcheck="false"
                      oninput={(e) => editRow(field.key, i, "paths", e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))} />
                  {:else}
                    <input value={String(row.path ?? "")} placeholder="field.path" spellcheck="false"
                      oninput={(e) => editRow(field.key, i, "path", e.currentTarget.value)} />
                    {#if row.op === "rename"}
                      <input value={String(row.to ?? "")} placeholder="new name" spellcheck="false"
                        oninput={(e) => editRow(field.key, i, "to", e.currentTarget.value)} />
                    {:else if row.op !== "remove"}
                      <input value={String(row.value ?? "")} placeholder={'value or {{ expression }}'} spellcheck="false"
                        oninput={(e) => editRow(field.key, i, "value", e.currentTarget.value)} />
                    {/if}
                  {/if}
                </div>
              {/each}
              <button class="btn btn-ghost btn-sm add-row"
                onclick={() => pushRow(field.key, { op: "set", path: "", value: "" })}>
                <Icon name="plus" size={12} /> Add operation
              </button>
            </div>
          {/if}
          {#if field.help}<span class="field-help">{field.help}</span>{/if}
        </label>
      {/if}
    {/each}
    {#if fields.length === 0}
      <p class="desc">This node has nothing to configure.</p>
    {/if}
  </div>

  <div class="section">
    <div class="mono-label">Node settings</div>
    <label class="field">
      <span class="field-label">If this node fails</span>
      <select bind:value={node.on_error} onchange={onchange}>
        <option value="stop">Stop the run</option>
        <option value="skip">Continue with nothing</option>
        <option value="branch">Route to an error output</option>
      </select>
    </label>
    <label class="field">
      <span class="field-label">Retries</span>
      <input type="number" min="0" max="5" value={node.retry?.attempts ?? 0}
        oninput={(e) => {
          const n = Math.max(0, Math.min(5, Number(e.currentTarget.value) || 0));
          node.retry = n === 0 ? null : { attempts: n, backoff_secs: [30, 120] };
          onchange();
        }} />
    </label>
    <label class="field toggle-field">
      <input type="checkbox" checked={node.disabled} onchange={(e) => { node.disabled = e.currentTarget.checked; onchange(); }} />
      <span>Disabled (passes items straight through)</span>
    </label>
    <label class="field">
      <span class="field-label">Notes</span>
      <textarea rows="2" bind:value={node.notes} oninput={onchange}></textarea>
    </label>
  </div>
</div>

<style>
  .inspector {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    padding: 0.9rem;
    overflow-y: auto;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .head-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--node-color) 16%, transparent);
    color: var(--node-color);
  }

  .head-text {
    flex: 1;
    min-width: 0;
  }

  .name-input {
    width: 100%;
    padding: 0.1rem 0.25rem;
    margin-left: -0.25rem;
    font-size: 0.9375rem;
    font-weight: 600;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
  }

  .name-input:hover,
  .name-input:focus {
    border-color: var(--border);
    background: var(--bg-surface-2);
    outline: none;
  }

  .head-kind {
    font-size: 0.6875rem;
    color: var(--text-faint);
  }

  .desc {
    font-size: 0.8125rem;
    color: var(--text-muted);
    line-height: 1.45;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    padding-top: 0.8rem;
    border-top: 1px solid var(--border);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .field-label {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .field-help {
    font-size: 0.6875rem;
    color: var(--text-faint);
  }

  .bad-note {
    color: var(--warning);
  }

  input:not([type="checkbox"]),
  select,
  textarea {
    width: 100%;
    padding: 0.45rem 0.6rem;
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.8125rem;
    transition: border-color var(--dur-fast) var(--ease);
  }

  input:focus,
  select:focus,
  textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  textarea {
    resize: vertical;
    line-height: 1.45;
  }

  .expr,
  .code {
    font-family: var(--font-mono);
    font-size: 0.75rem;
  }

  .code.bad {
    border-color: var(--warning);
  }

  .toggle-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .toggle-hint {
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  .toggle-field {
    flex-direction: row;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--text-secondary);
  }

  .kv {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .kv-row {
    display: grid;
    grid-template-columns: 1fr 1.4fr auto;
    gap: 0.35rem;
    align-items: center;
  }

  .cond-row {
    display: grid;
    grid-template-columns: 1.3fr auto 1fr auto;
    gap: 0.35rem;
    align-items: center;
  }

  .cond-row select {
    width: auto;
  }

  .op-row {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
  }

  .op-head {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .op-head select {
    flex: 1;
  }

  .row-x {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    flex: none;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
  }

  .row-x:hover {
    background: var(--bg-hover);
    color: var(--danger);
  }

  .add-row {
    align-self: flex-start;
  }
</style>
