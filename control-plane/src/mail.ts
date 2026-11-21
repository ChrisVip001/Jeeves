export type MailAction = "verify" | "reset";
export type AccountMailer = (
  email: string,
  action: MailAction,
  token: string,
) => Promise<void | { messageId: string }>;

export function accountMessage(
  origin: string,
  email: string,
  action: MailAction,
  token: string,
) {
  const link = new URL("/account/", origin);
  link.hash = new URLSearchParams({ action, email, token }).toString();
  const subject =
    action === "verify"
      ? "Verify your Jeeves email · 验证邮箱"
      : "Reset your Jeeves password · 重设密码";
  return {
    subject,
    text:
      subject +
      "\n\n" +
      link.href +
      "\n\nValid for 15 minutes. Ignore if you did not request this.\n链接 15 分钟内有效。如非本人操作，请忽略。",
  };
}

export function accountMailer(
  origin: string,
  from: string,
  key: string,
  send = fetch,
): AccountMailer {
  return async (email, action, token) => {
    const { subject, text } = accountMessage(origin, email, action, token);
    const reply = await send("https://api.resend.com/emails", {
      method: "POST",
      redirect: "error",
      signal: AbortSignal.timeout(10000),
      headers: {
        authorization: "Bearer " + key,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        from,
        to: [email],
        subject,
        text,
      }),
    });
    if (!reply.ok) throw new Error("account mail delivery failed");
  };
}
