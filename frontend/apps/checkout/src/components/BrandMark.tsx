export function BrandMark({ size = 24 }: { size?: number }) {
  return (
    <p className="pay-brand">
      <img src="/logo.svg" alt="" width={size} height={size} />
      <span>tachyon</span>
    </p>
  );
}
