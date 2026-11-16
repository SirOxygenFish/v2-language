export interface Point { x: number; y: number }
export type Shape = 'circle' | 'square';
export type ShapeList = Shape[];
interface Hidden { z: number }
export function dist(p: Point): number { return Math.hypot(p.x, p.y); }
