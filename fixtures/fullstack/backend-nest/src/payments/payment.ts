export interface PaymentGateway {
  charge(amount: number): Promise<boolean>;
}

export class StripeGateway implements PaymentGateway {
  async charge(amount: number): Promise<boolean> {
    const res = await fetch("https://api.stripe.com/v1/charges", { method: "POST" });
    return res.ok && amount > 0;
  }
}
