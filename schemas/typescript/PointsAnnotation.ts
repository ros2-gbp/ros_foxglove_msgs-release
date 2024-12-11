// Generated by https://github.com/foxglove/schemas
// Options: {}

import { Color } from "./Color";
import { Point2 } from "./Point2";
import { PointsAnnotationType } from "./PointsAnnotationType";
import { Time } from "./Time";

/** An array of points on a 2D image */
export type PointsAnnotation = {
  /** Timestamp of annotation */
  timestamp: Time;

  /** Type of points annotation to draw */
  type: PointsAnnotationType;

  /**
   * Points in 2D image coordinates (pixels).
   * These coordinates use the top-left corner of the top-left pixel of the image as the origin.
   */
  points: Point2[];

  /** Outline color */
  outline_color: Color;

  /** Per-point colors, if `type` is `POINTS`, or per-segment stroke colors, if `type` is `LINE_LIST`, `LINE_STRIP` or `LINE_LOOP`. */
  outline_colors: Color[];

  /** Fill color */
  fill_color: Color;

  /** Stroke thickness in pixels */
  thickness: number;
};
