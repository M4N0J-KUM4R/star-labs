<script lang="ts">
	import { goto } from '$app/navigation';
	import { session } from '$lib/auth.svelte';
	import { ApiError } from '$lib/api';

	let email = $state('');
	let password = $state('');
	let busy = $state(false);
	let failure = $state<string | null>(null);

	async function submit() {
		busy = true;
		failure = null;
		try {
			await session.signIn(email.trim(), password);
			await goto('/dashboard');
		} catch (error) {
			/* The API answers 401 with a plain-text body and a variant prefix, so
			 * there is nothing to parse — show the safe message, never the raw body. */
			failure =
				error instanceof ApiError && error.status === 401
					? 'Could not log you in. Check your email and password.'
					: 'The server could not be reached.';
		} finally {
			busy = false;
		}
	}
</script>

<!-- Canvas' login page root is .ic-Login (app/views/shared/_auth_type_icon.html.erb
     references it), so the vendored login bundle centers and paints this form. -->
<div class="ic-Login">
	<div class="ic-Login-content">
		<h1 class="ic-Login__title">Log In</h1>

		{#if failure}
			<div class="ic-flash-error" role="alert">
				<h4 class="ic-flash__headline">{failure}</h4>
			</div>
		{/if}

		<form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
			<div class="ic-Login-input-container">
				<label class="ic-Login-label" for="credential_email">Email</label>
				<input
					class="ic-Login-input"
					id="credential_email"
					name="email"
					type="email"
					autocomplete="username"
					required
					bind:value={email}
				/>
			</div>
			<div class="ic-Login-input-container">
				<label class="ic-Login-label" for="credential_password">Password</label>
				<input
					class="ic-Login-input"
					id="credential_password"
					name="password"
					type="password"
					autocomplete="current-password"
					required
					bind:value={password}
				/>
			</div>
			<div class="ic-Login-submit-button-container">
				<button class="btn btn-primary ic-Login-submit-button" type="submit" disabled={busy}>
					{busy ? 'Logging in...' : 'Log In'}
				</button>
			</div>
		</form>

		<p class="ic-Login-footer">
			New here? <a href="/register">Create an account</a>
		</p>
	</div>
</div>
