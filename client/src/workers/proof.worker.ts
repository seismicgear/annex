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

async function assertProofAssetAvailable(path: string): Promise<void> {
  let response: Response;
  try {
    response = await fetch(path, { method: 'GET', cache: 'no-store' });
  } catch {
    const err = new Error(`Required proof asset could not be fetched: ${path}.`);
    err.name = 'ZkProofAssetsError';
    throw err;
  }

  if (!response.ok) {
    const err = new Error(`Required proof asset is unavailable (${response.status}): ${path}.`);
    err.name = 'ZkProofAssetsError';
    throw err;
  }
}

self.onmessage = async (event: MessageEvent<StartMessage>) => {
  const message = event.data;
  if (!message || message.kind !== 'start') return;

  try {
    emitStatus(message.jobId, 'loading_assets');
    await assertProofAssetAvailable(message.wasmPath);
    await assertProofAssetAvailable(message.zkeyPath);

    emitStatus(message.jobId, 'computing_witness');
    emitStatus(message.jobId, 'generating_proof');

    const { proof, publicSignals } = await snarkjs.groth16.fullProve(
      message.input,
      message.wasmPath,
      message.zkeyPath,
    );

    self.postMessage({ kind: 'result', jobId: message.jobId, proof, publicSignals });
  } catch (error) {
    const err = error instanceof Error ? error : new Error(String(error));
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
