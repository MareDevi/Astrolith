/**
 * Static-analysis fixture for the JSX parser. It is never rendered.
 */
export default function Toggle({ checked = false, label, onChange }) {
  return (
    <label>
      <input type="checkbox" checked={checked} onChange={onChange} />
      <span>{label}</span>
    </label>
  );
}
