import { redirect } from '@sveltejs/kit';

// Canvas sends an authenticated visitor at `/` to the dashboard. With ssr=false
// this load runs in the browser, which is where the session lives anyway.
redirect(303, '/dashboard');
