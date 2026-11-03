import type { ButtonProps } from "./button-props";

const Button: React.FC<ButtonProps> = ({ label, disabled = false }) => (
  <button disabled={disabled}>{label}</button>
);

export default Button;
