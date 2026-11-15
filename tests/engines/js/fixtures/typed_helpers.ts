export interface Point {
  x: number;
  y: number;
}

export function norm(p: Point): number {
  return Math.sqrt(p.x * p.x + p.y * p.y);
}

export const label: string = "typed";
