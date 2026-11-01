/**
 * Static-analysis fixture for the component builder's JSX parser. It is never
 * rendered by this Astro project.
 */
export default function Toggle({ checked = false, label, onChange }) {
  return (
    <label>
      <input type="checkbox" checked={checked} onChange={onChange} />
      <span>{label}</span>
    </label>
  );
}
