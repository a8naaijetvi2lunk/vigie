import mark from "../assets/vigie-mark.svg";

export default function Brand({ active = false }: { active?: boolean }) {
  return <span className="brand" data-active={active}><img src={mark} alt="" /><span>Vigie</span></span>;
}
