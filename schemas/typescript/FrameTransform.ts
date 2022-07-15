// Generated from FrameTransform by @foxglove/schemas

import { Quaternion } from "./Quaternion";
import { Time } from "./Time";
import { Vector3 } from "./Vector3";

/** A transform between two reference frames in 3D space */
export type FrameTransform = {
  /** Timestamp of transform */
  timestamp: Time;

  /** Name of the parent frame */
  parent_frame_id: string;

  /** Name of the child frame */
  child_frame_id: string;

  /** Translation component of the transform */
  translation: Vector3;

  /** Rotation component of the transform */
  rotation: Quaternion;
};