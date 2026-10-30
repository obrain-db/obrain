export function ChoiceChips<T extends string | number>(p: {
  options: { value: T; label: string }[]; value: T; onChange: (v: T) => void; name?: string;
}){
  return <div className="chips">
    {p.options.map(o => <button key={String(o.value)}
      className={p.value === o.value ? "chip on" : "chip"}
      onClick={() => p.onChange(o.value)}>{o.label}</button>)}
  </div>;
}
export function Toggle(p: { label: string; on: boolean; onToggle: () => void }){
  return <label className="tog"><input type="checkbox" checked={p.on} onChange={p.onToggle}/>{p.label}</label>;
}
