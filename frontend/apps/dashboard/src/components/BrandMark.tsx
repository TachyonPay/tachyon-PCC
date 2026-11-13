export function BrandMark({ href = "/", size = 24 }: { href?: string | null; size?: number }) {
  const inner = (
    <>
      <img src="/logo.svg" alt="" width={size} height={size} />
      <span>tachyon</span>
    </>
  );
  if (!href) return <p className="brand-mark">{inner}</p>;
  return (
    <a href={href} className="brand-mark">
      {inner}
    </a>
  );
}
