export function loadConfig(env: NodeJS.ProcessEnv = process.env) {
  const required = (name: string) => {
    const value = env[name];
    if (!value) throw new Error(`Missing ${name}`);
    return value;
  };
  return {
    database: required('DATABASE_URL'),
    port: Number(env.PORT ?? 3000),
    origins: (env.WEB_ORIGINS ?? '').split(','),
  };
}

export function loadHealth(env: Record<string, string | undefined>) {
  // A loosely typed map named `env` counts only for CONSTANT_CASE keys.
  return env.HEALTH_URL ?? env.someField;
}

export function notEnv(options: Record<string, string>) {
  return options.SHOULD_NOT_COUNT;
}
