export declare function formatStamp(epochMs: number): string;
export declare function createLineStamper(now?: () => number): (chunk: Uint8Array) => Buffer;
