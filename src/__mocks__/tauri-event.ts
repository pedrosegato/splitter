export const listen = async () => () => {};
export const once = async () => () => {};
export const emit = async () => {};
export type EventCallback<T> = (event: { payload: T }) => void;
