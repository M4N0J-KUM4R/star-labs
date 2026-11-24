import '@sveltejs/kit';

declare global {
	namespace App {
		interface Error {
			message: string;
			status?: number;
		}
		// interface PageData {}
		// interface Platform {}
	}
}

export {};
