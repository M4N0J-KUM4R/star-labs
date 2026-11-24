<script lang="ts">
	import { goto } from '$app/navigation';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import { setToken } from '$lib/token';

	let form = $state({ name: '', email: '', password: '', confirm: '' });
	let busy = $state(false);
	let failure = $state<string | null>(null);

	let mismatch: boolean = $derived(
		form.confirm.length > 0 && form.confirm !== form.password
	);

	async function submit() {
		busy = true;
		failure = null;
		try {
			const result = await endpoints.session.register({
				name: form.name.trim(),
				email: form.email.trim(),
				password: form.password
			});
			/* star-auth's register returns the same { token, user } as login, so a
			 * new account is signed in immediately rather than bounced to /login. */
			setToken(result.token);
			session.user = result.user;
			await goto('/dashboard');
		} catch (error) {
			failure = (error as { publicMessage?: string }).publicMessage ?? 'Registration failed.';
		} finally {
			busy = false;
		}
	}
</script>

<div class="ic-Login">
	<div class="ic-Login-content">
		<h1 class="ic-Login__title">Create an account</h1>

		{#if failure}
			<div class="ic-flash-error" role="alert">
				<h4 class="ic-flash__headline">{failure}</h4>
			</div>
		{/if}

		<form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
			<p>
				<label for="reg_name">Full name</label>
				<input id="reg_name" name="name" required bind:value={form.name} />
			</p>
			<p>
				<label for="reg_email">Email</label>
				<input id="reg_email" name="email" type="email" required bind:value={form.email} />
			</p>
			<p>
				<label for="reg_password">Password</label>
				<input
					id="reg_password"
					name="password"
					type="password"
					autocomplete="new-password"
					required
					bind:value={form.password}
				/>
			</p>
			<p>
				<label for="reg_confirm">Confirm password</label>
				<input
					id="reg_confirm"
					name="password_confirmation"
					type="password"
					autocomplete="new-password"
					required
					bind:value={form.confirm}
				/>
				{#if mismatch}
					<span class="ic-flash-error">Passwords do not match.</span>
				{/if}
			</p>
			<button class="btn btn-primary" type="submit" disabled={busy || mismatch}>
				{busy ? 'Creating...' : 'Register'}
			</button>
		</form>

		<p class="ic-Login-footer">
			Already registered? <a href="/login">Log in</a>
		</p>
	</div>
</div>
