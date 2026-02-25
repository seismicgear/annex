import * as snarkjs from 'snarkjs';

type StartMessage = {
  kind: 'start';
  jobId: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  input: any;
  wasmPath: string;
  zkeyPath: string;
};

function emitStatus(jobId: string, stage: 'loading_assets' | 'computing_witness' | 'generating_proof'): void {
  self.postMessage({ kind: 'status', jobId, stage });
}

function isLikelyProofAssetFailure(error: Error, wasmPath: string, zkeyPath: string): boolean {
  const message = error.message.toLowerCase();
  const hasAssetPath = message.includes(wasmPath.toLowerCase()) || message.includes(zkeyPath.toLowerCase());
  const hasAssetHint =
    message.includes('wasm')
    || message.includes('zkey')
    || message.includes('failed to fetch')
    || message.includes('networkerror')
    || message.includes('not found')
    || message.includes('status code 404')
    || message.includes('enoent');

  return hasAssetPath || hasAssetHint;
}

function mapProofError(error: unknown, wasmPath: string, zkeyPath: string): Error {
  const err = error instanceof Error ? error : new Error(String(error));
  if (err.name === 'ZkProofAssetsError' || isLikelyProofAssetFailure(err, wasmPath, zkeyPath)) {
    const mappedError = new Error(
      `Required proof asset could not be loaded. Ensure both assets are available: ${wasmPath} and ${zkeyPath}.`,
    );
    mappedError.name = 'ZkProofAssetsError';
    return mappedError;
  }

  return err;
}

self.onmessage = async (event: MessageEvent<StartMessage>) => {
  const message = event.data;
  if (!message || message.kind !== 'start') return;

  try {
    emitStatus(message.jobId, 'loading_assets');

    emitStatus(message.jobId, 'computing_witness');
    emitStatus(message.jobId, 'generating_proof');

    const { proof, publicSignals } = await snarkjs.groth16.fullProve(
      message.input,
      message.wasmPath,
      message.zkeyPath,
    );

    self.postMessage({ kind: 'result', jobId: message.jobId, proof, publicSignals });
  } catch (error) {
    const err = mapProofError(error, message.wasmPath, message.zkeyPath);
    self.postMessage({
      kind: 'error',
      jobId: message.jobId,
      error: {
        name: err.name,
        message: err.message,
      },
    });
  }
};
